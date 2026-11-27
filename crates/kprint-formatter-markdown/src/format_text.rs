use std::ops::Range;

use kprint_configuration::resolve_new_line_kind;
use kprint_formatting::*;

use super::EmbeddedCode;
use super::configuration::Configuration;
use super::generation::Context;
use super::generation::generate;
use super::generation::is_ignore_comment;
use super::parser::strip_metadata_header;

/// Error that can occur while formatting markdown text.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
  /// The text could not be parsed as markdown.
  ///
  /// Whatever the parser can't make sense of is read as the paragraphs it
  /// renders as, so this is only for a document written in a way the parser
  /// won't follow at all -- blocks nested more deeply than it will go.
  #[error("{0}")]
  Parse(String),
}

/// Formats a file.
///
/// Returns the formatted text, or `None` when it's the same as what was given.
pub fn format_text(
  file_text: &str,
  config: &Configuration,
  format_code_block_text: impl FnMut(EmbeddedCode<'_>) -> Option<String>,
) -> Result<Option<String>, FormatError> {
  format_text_with_arena_pool(file_text, config, None, format_code_block_text)
}

/// Formats a file, creating the arena the parsed file and the print items are
/// allocated in from `arena_pool` when there is one so that its memory is reused
/// between formats.
pub(crate) fn format_text_with_arena_pool(
  file_text: &str,
  config: &Configuration,
  arena_pool: Option<&ArenaPool>,
  format_code_block_text: impl FnMut(EmbeddedCode<'_>) -> Option<String>,
) -> Result<Option<String>, FormatError> {
  let result = format_text_inner(file_text, config, None, arena_pool, format_code_block_text)?;

  match result {
    Some(result) if result == file_text => Ok(None),
    Some(result) => Ok(Some(result)),
    None => Ok(None),
  }
}

/// Formats a file, only formatting the code blocks within `code_block_range`
/// when there is one.
pub(crate) fn format_text_inner(
  file_text: &str,
  config: &Configuration,
  code_block_range: Option<Range<usize>>,
  arena_pool: Option<&ArenaPool>,
  format_code_block_text: impl FnMut(EmbeddedCode<'_>) -> Option<String>,
) -> Result<Option<String>, FormatError> {
  let body = strip_bom(file_text);
  let arena = match arena_pool {
    Some(arena_pool) => arena_pool.create_arena(),
    None => Arena::new(),
  };
  let (source_file, markdown_text) = match parse_source_file(body, config, arena.bump())? {
    ParseFileResult::IgnoreFile => return Ok(None),
    ParseFileResult::SourceFile(file) => file,
  };
  let mut context = Context::new(
    &arena,
    markdown_text,
    config,
    (source_file.link_labels, source_file.footnote_labels),
    format_code_block_text,
    file_text.len() - body.len(),
  );
  context.code_block_range = code_block_range;
  let print_items = generate(&source_file.into(), &mut context);
  Ok(Some(kprint_formatting::print(print_items, config_to_print_options(body, config))))
}

#[cfg(feature = "tracing")]
pub fn trace_file(
  file_text: &str,
  config: &Configuration,
  format_code_block_text: impl FnMut(EmbeddedCode<'_>) -> Option<String>,
) -> kprint_formatting::TracingResult {
  let arena = Arena::new();
  let body = strip_bom(file_text);
  let (source_file, markdown_text) = match parse_source_file(body, config, arena.bump()).unwrap() {
    ParseFileResult::IgnoreFile => panic!("Cannot trace file because it has an ignore file comment."),
    ParseFileResult::SourceFile(file) => file,
  };
  let mut context = Context::new(
    &arena,
    markdown_text,
    config,
    (source_file.link_labels, source_file.footnote_labels),
    format_code_block_text,
    file_text.len() - body.len(),
  );
  let print_items = generate(&source_file.into(), &mut context);
  kprint_formatting::trace_printing(print_items, config_to_print_options(body, config))
}

/// Removes the byte order marks a file begins with, along with any whitespace
/// written before them.
///
/// More than one is removed, and the whitespace with them, because taking off
/// only the first would leave another at the start of the file once the
/// whitespace around it was written out -- for the next run to take off in
/// turn. Text that begins with no mark at all is left as it is.
pub(crate) fn strip_bom(text: &str) -> &str {
  let mut rest = text;
  loop {
    let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r']);
    match trimmed.strip_prefix('\u{FEFF}') {
      Some(after) => rest = after,
      None => return rest,
    }
  }
}

enum ParseFileResult<'a> {
  IgnoreFile,
  SourceFile((crate::generation::common::SourceFile<'a>, &'a str)),
}

fn parse_source_file<'a>(file_text: &'a str, config: &Configuration, bump: &'a bumpalo::Bump) -> Result<ParseFileResult<'a>, FormatError> {
  // check for the presence of a dprint-ignore-file comment before parsing
  if is_ignore_comment(strip_metadata_header(file_text, bump), &config.ignore_file_directive) {
    return Ok(ParseFileResult::IgnoreFile);
  }

  let file = crate::parser::parse(file_text, bump).map_err(|err| FormatError::Parse(err.to_string()))?;
  Ok(ParseFileResult::SourceFile((file, file_text)))
}

fn config_to_print_options(file_text: &str, config: &Configuration) -> PrintOptions {
  PrintOptions {
    indent_width: 1, // force
    max_width: config.line_width,
    use_tabs: false, // ignore tabs, always use spaces
    new_line_text: resolve_new_line_kind(file_text, config.new_line_kind),
  }
}

#[cfg(test)]
mod test {
  use super::*;
  use crate::configuration::ConfigurationBuilder;

  #[test]
  fn preserves_significant_inline_text_and_html_whitespace() {
    let config = ConfigurationBuilder::new().ignore_directive("freeze").html_skip_format(true).build();
    let input = concat!(
      "Prefix <!-- freeze --> suffix\n\n",
      "`a\tb`\n\n",
      "[x](<> \"tip\") and ![x](<> \"tip\")\n\n",
      "<pre>alpha  \nbeta</pre>\n",
    );
    assert_eq!(format_text(input, &config, |_| None).unwrap(), None);
  }

  #[test]
  fn does_not_absorb_outside_text_after_a_quoted_table_or_link_definition() {
    for input in ["> a | b\n> - | -\n> c | d\noutside\n", "> [ref]: /url\noutside\n"] {
      let formatted = format_text(input, &Configuration::default(), |_| None).unwrap();
      assert!(formatted.as_deref().unwrap_or(input).ends_with("\noutside\n"), "{formatted:?}");
    }
  }

  #[test]
  fn keeps_a_pipe_only_row_inside_the_table() {
    let input = "| h |\n| --- |\n|\n| data |\n";
    let output = format_text(input, &Configuration::default(), |_| None).unwrap().unwrap();
    assert_eq!(output.lines().count(), 4);
    assert!(output.lines().all(|line| line.starts_with('|')), "{output}");
  }

  #[test]
  fn strips_bom() {
    for input_text in [
      "\u{FEFF}#  Title",
      "\u{FEFF}# Title\n",
      "\u{FEFF}\u{FEFF}# Title\n",
      "  \u{FEFF}# Title\n",
      "\u{FEFF}\n\u{FEFF}# Title\n",
    ] {
      let config = ConfigurationBuilder::new().build();
      let result = format_text(input_text, &config, |_| None).unwrap();
      assert_eq!(result, Some("# Title\n".to_string()));
    }
  }

  #[test]
  fn empty_and_whitespace_only_files() {
    let config = ConfigurationBuilder::new().build();
    assert_eq!(format_text("", &config, |_| None).unwrap(), None);
    for input_text in ["\n", "\n\n\n", "   \n  \n", "\u{FEFF}", "\u{FEFF}\n"] {
      let result = format_text(input_text, &config, |_| None).unwrap();
      assert_eq!(result, Some(String::new()), "{:?}", input_text);
    }
  }

  #[test]
  fn ends_the_file_with_a_single_newline() {
    let config = ConfigurationBuilder::new().build();
    for (input_text, expected) in [("a", Some("a\n")), ("a\n", None), ("a\n\n", Some("a\n"))] {
      let result = format_text(input_text, &config, |_| None).unwrap();
      assert_eq!(result.as_deref(), expected, "{:?}", input_text);
    }
  }

  #[test]
  fn new_line_kind() {
    let config = ConfigurationBuilder::new().build();
    let result = format_text("a\r\nb\r\n", &config, |_| None).unwrap();
    assert_eq!(result.as_deref(), Some("a\nb\n"));

    let config = ConfigurationBuilder::new().new_line_kind(kprint_configuration::NewLineKind::Auto).build();
    for (input_text, expected) in [
      ("a\r\nb\r\n", None),
      ("a\nb\r\n", Some("a\r\nb\r\n")),
      ("a\r\nb", Some("a\r\nb\r\n")),
      ("\u{FEFF}a\r\nb\r\n", Some("a\r\nb\r\n")),
    ] {
      let result = format_text(input_text, &config, |_| None).unwrap();
      assert_eq!(result.as_deref(), expected, "{:?}", input_text);
    }
  }

  #[test]
  fn ignore_file_directive_variants() {
    let config = ConfigurationBuilder::new().build();
    for input_text in [
      "<!-- dprint-ignore-file -->\n#  a",
      "\u{FEFF}<!-- dprint-ignore-file -->\n#  a",
      "<!-- dprint-ignore-file -->\r\n#  a",
      "---\na: b\n---\n\n<!-- dprint-ignore-file -->\n#  a",
    ] {
      let result = format_text(input_text, &config, |_| None).unwrap();
      assert_eq!(result, None, "{:?}", input_text);
    }
  }

  #[test]
  fn code_block_callback_return_values() {
    let config = ConfigurationBuilder::new().build();
    let input_text = "```js\nx\n```\n";
    for returned in ["formatted", "formatted\n", "formatted\r\n", "formatted\n\n"] {
      let result = format_text(input_text, &config, |_| Some(returned.to_string())).unwrap();
      assert_eq!(result.as_deref(), Some("```js\nformatted\n```\n"), "{:?}", returned);
    }
    for returned in ["", "\n"] {
      let result = format_text(input_text, &config, |_| Some(returned.to_string())).unwrap();
      assert_eq!(result.as_deref(), Some("```js\n```\n"), "{:?}", returned);
    }
    let result = format_text(input_text, &config, |block| Some(block.text.to_string())).unwrap();
    assert_eq!(result, None);
  }

  #[test]
  fn code_block_callback_receives_tag_text_and_width() {
    let config = ConfigurationBuilder::new().line_width(12).build();
    let mut calls = Vec::new();
    let input_text = "```rust,ignore\nx\n```\n\n```JS\r\ny\r\nz\r\n```\r\n\n> - ```ts\n>   w\n>   ```\n";
    format_text(
      input_text,
      &config,
      |EmbeddedCode {
         language: tag,
         text,
         line_width: width,
         ..
       }| {
        calls.push(format!("{} {:?} {}", tag, text, width));
        None
      },
    )
    .unwrap();
    // the tag is passed as written and the width is what is left after the
    // indentation, though never below 10
    assert_eq!(calls, vec!["rust \"x\" 12", "JS \"y\\nz\" 12", "ts \"w\" 10"]);
  }

  #[test]
  fn code_block_callback_not_called() {
    let mut calls = 0;
    let mut count = |_: EmbeddedCode<'_>| {
      calls += 1;
      None
    };
    let config = ConfigurationBuilder::new().code_block_skip_format(true).build();
    assert_eq!(format_text("```js\nx\n```\n", &config, &mut count).unwrap(), None);
    let config = ConfigurationBuilder::new().build();
    for input_text in [
      "<!-- dprint-ignore-start -->\n\n```js\nx\n```\n\n<!-- dprint-ignore-end -->\n",
      "```\nx\n```\n",
      "    x\n",
    ] {
      assert_eq!(format_text(input_text, &config, &mut count).unwrap(), None, "{:?}", input_text);
    }
    assert_eq!(calls, 0);
  }

  #[test]
  fn nested_markdown_reports_the_outer_tag() {
    let config = ConfigurationBuilder::new().build();
    for outer_tag in ["markdown", "md", "MD", "Markdown"] {
      let input_text = format!("````{}\n#  Title\n\n```js\nx\n```\n````\n", outer_tag);
      let mut tags = Vec::new();
      let result = format_text(&input_text, &config, |block| {
        tags.push(block.language.to_string());
        None
      })
      .unwrap();
      assert_eq!(tags, vec![outer_tag]);
      assert_eq!(result, None);
    }
  }

  #[test]
  fn preserve_options_pass_text_through() {
    let config = ConfigurationBuilder::new().code_block_preserve_indentation(true).build();
    let mut seen = Vec::new();
    format_text("```js\n    x\n```\n", &config, |block| {
      seen.push(block.text.to_string());
      None
    })
    .unwrap();
    assert_eq!(seen, vec!["    x"]);

    let config = ConfigurationBuilder::new().code_block_preserve_blank_lines(true).build();
    let mut seen = Vec::new();
    format_text("```js\n\nx\n\n```\n", &config, |block| {
      seen.push(block.text.to_string());
      None
    })
    .unwrap();
    assert_eq!(seen, vec!["\nx\n"]);
  }
}
