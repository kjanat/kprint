use std::collections::HashMap;
use std::ops::Range;

use kprint_plugin_types::FormatRange;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::DidChangeTextDocumentParams;
use tower_lsp::lsp_types::DidCloseTextDocumentParams;
use tower_lsp::lsp_types::TextDocumentItem;
use url::Url;

use crate::environment::HostEnvironment as Environment;

use super::text::LineIndex;

#[derive(Debug, PartialEq, Eq)]
enum IndexValid {
  All,
  UpTo(u32),
}

impl IndexValid {
  fn covers(&self, line: u32) -> bool {
    match *self {
      IndexValid::UpTo(to) => to > line,
      IndexValid::All => true,
    }
  }
}

pub struct Document {
  line_index: Option<LineIndex>,
  version: i32,
  #[allow(dead_code)]
  pub language_id: String,
  pub text: String,
}

pub struct Documents<TEnvironment: Environment> {
  environment: TEnvironment,
  docs: HashMap<Url, Document>,
}

impl<TEnvironment: Environment> Documents<TEnvironment> {
  pub fn new(environment: TEnvironment) -> Self {
    Self {
      environment,
      docs: Default::default(),
    }
  }

  pub fn open(&mut self, text_document_item: TextDocumentItem) {
    self.docs.insert(
      text_document_item.uri.clone(),
      Document {
        line_index: None,
        language_id: text_document_item.language_id,
        version: text_document_item.version,
        text: text_document_item.text,
      },
    );
  }

  pub fn get_content(&self, uri: &Url) -> Option<(String, Option<LineIndex>)> {
    let Some(entry) = self.docs.get(uri) else {
      log_warn!(self.environment, "Missing document: {}", uri);
      return None;
    };
    Some((entry.text.clone(), entry.line_index.clone()))
  }

  pub fn get_content_with_range(&mut self, uri: &Url, lsp_range: lsp_types::Range) -> Option<(String, FormatRange, LineIndex)> {
    let Some(entry) = self.docs.get_mut(uri) else {
      log_warn!(self.environment, "Missing document: {}", uri);
      return None;
    };

    let line_index = entry.line_index.get_or_insert_with(|| LineIndex::new(&entry.text));
    let range = line_index.get_text_range(lsp_range).ok()?;
    Some((entry.text.clone(), Some(range.start().into()..range.end().into()), line_index.clone()))
  }

  pub fn changed(&mut self, params: DidChangeTextDocumentParams) {
    let Some(entry) = self.docs.get_mut(&params.text_document.uri) else {
      log_warn!(self.environment, "Missing document: {}", params.text_document.uri);
      return;
    };
    if entry.version > params.text_document.version {
      // the state has gone out of sync so it's no longer safe to format this document
      log_warn!(
        self.environment,
        "Changed version ({}) was less than existing version ({}) for '{}'. Forgetting document.",
        params.text_document.version,
        entry.version,
        params.text_document.uri,
      );
      self.docs.remove(&params.text_document.uri);
      return;
    }
    let mut content = entry.text.to_string();
    let mut line_index = entry.line_index.take().unwrap_or_else(|| LineIndex::new(&content));
    let mut index_valid = IndexValid::All;
    for change in params.content_changes {
      if let Some(range) = change.range {
        if !index_valid.covers(range.end.line) {
          line_index = LineIndex::new(&content);
        }
        index_valid = IndexValid::UpTo(range.start.line.min(line_index.last_line()));
        let range = match line_index.get_text_range(range) {
          Ok(range) => range,
          Err(err) => {
            log_warn!(self.environment, "Had error for '{}'. Forgetting document. {:#}", params.text_document.uri, err);
            self.docs.remove(&params.text_document.uri);
            return;
          }
        };
        content.replace_range(Range::<usize>::from(range), &change.text);
      } else {
        content = change.text;
        index_valid = IndexValid::UpTo(0);
      }
    }
    if index_valid == IndexValid::All {
      entry.line_index = Some(line_index);
    }
    entry.text = content;
    entry.version = params.text_document.version;
  }

  pub fn closed(&mut self, params: DidCloseTextDocumentParams) {
    self.docs.remove(&params.text_document.uri);
  }
}

#[cfg(test)]
mod test {
  use std::str::FromStr;
  use url::Url as Uri;

  use tower_lsp::lsp_types::Position;
  use tower_lsp::lsp_types::TextDocumentContentChangeEvent;
  use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;

  use crate::environment::TestEnvironment;

  use super::*;

  #[test]
  fn ranged_changes_in_one_notification_moving_down() {
    // the second change is below the first one, so the offsets of its lines
    // moved because of the first change
    assert_eq!(
      apply_changes("a\nb\nc\n", vec![ranged((0, 0), (0, 1), "aaaa\naa"), ranged((3, 0), (3, 1), "C")]),
      "aaaa\naa\nb\nC\n"
    );
    // on the same line as the first change
    assert_eq!(
      apply_changes("a\nb\nc\n", vec![ranged((1, 0), (1, 1), "bbbb"), ranged((1, 3), (1, 4), "X")]),
      "a\nbbbX\nc\n"
    );
  }

  #[test]
  fn ranged_changes_in_one_notification_moving_up() {
    // entirely above the first change
    assert_eq!(
      apply_changes("a\nb\nc\n", vec![ranged((2, 0), (2, 1), "cccc"), ranged((0, 0), (0, 1), "A")]),
      "A\nb\ncccc\n"
    );
    // starts above the first change and ends on its line
    assert_eq!(
      apply_changes("a\nb\nc\n", vec![ranged((1, 0), (1, 1), "bbbb"), ranged((0, 0), (1, 4), "X")]),
      "X\nc\n"
    );
    // starts above the first change and ends below it
    assert_eq!(
      apply_changes("a\nb\nc\n", vec![ranged((1, 0), (1, 1), "bbbb"), ranged((0, 0), (2, 1), "X")]),
      "X\n"
    );
    assert_eq!(
      apply_changes(
        "a\nb\nc\nd\n",
        vec![ranged((2, 0), (2, 1), "c\nc\nc"), ranged((1, 0), (1, 1), "bbbb"), ranged((0, 1), (5, 1), "X")]
      ),
      "aX\n"
    );
  }

  #[test]
  fn ranged_change_after_change_starting_past_last_line() {
    // the first change is at the end of the text, which is on the last line,
    // so the second change can't use the line index for that line
    assert_eq!(apply_changes("a\nb", vec![ranged((5, 0), (5, 0), "xyz"), ranged((1, 0), (1, 4), "")]), "a\n");
    assert_eq!(apply_changes("a\nb", vec![ranged((2, 0), (2, 0), "xyz"), ranged((1, 0), (1, 4), "")]), "a\n");
    // the last line is the empty one after the final newline
    assert_eq!(
      apply_changes("a\nb\n", vec![ranged((3, 0), (3, 0), "xyz"), ranged((2, 0), (2, 3), "Q")]),
      "a\nb\nQ"
    );
    // starts above the last line and ends on it
    assert_eq!(apply_changes("a\nb\nc", vec![ranged((9, 0), (9, 0), "xyz"), ranged((1, 0), (2, 4), "")]), "a\n");
    // the lines above the last one are still found after such a change
    assert_eq!(
      apply_changes(
        "a\nb\nc",
        vec![ranged((9, 0), (9, 0), "\nd"), ranged((1, 0), (1, 1), "B"), ranged((0, 0), (0, 1), "A")]
      ),
      "A\nB\nc\nd"
    );
    // empty text
    assert_eq!(apply_changes("", vec![ranged((4, 0), (4, 0), "xyz"), ranged((0, 0), (0, 3), "Q")]), "Q");
  }

  #[test]
  fn ranged_change_after_surrogate_pair() {
    // the columns are in utf-16 code units and the emoji is two of them
    assert_eq!(apply_changes("a😀b\nc\n", vec![ranged((0, 3), (0, 4), "X")]), "a😀X\nc\n");
    assert_eq!(
      apply_changes("ab\nc\n", vec![ranged((0, 1), (0, 1), "😀"), ranged((0, 3), (0, 4), "X")]),
      "a😀X\nc\n"
    );
    // a change above one that added a surrogate pair
    assert_eq!(
      apply_changes("a\nb\nc\n", vec![ranged((1, 0), (1, 0), "😀"), ranged((0, 0), (1, 3), "X")]),
      "X\nc\n"
    );
  }

  #[test]
  fn full_text_change_mixed_with_ranged_changes() {
    assert_eq!(
      apply_changes(
        "a\nb\nc\n",
        vec![
          ranged((2, 0), (2, 1), "cccc"),
          full("one\ntwo\nthree\nfour\n"),
          ranged((3, 0), (3, 4), "4"),
          ranged((0, 0), (1, 3), "1\n2"),
        ]
      ),
      "1\n2\nthree\n4\n"
    );
    assert_eq!(apply_changes("a\nb\nc\n", vec![ranged((0, 0), (0, 1), "A"), full("one\ntwo\n")]), "one\ntwo\n");
  }

  #[test]
  fn ranged_changes_with_cached_line_index() {
    let (mut documents, uri) = open_document("a\nb\nc\n");
    // this caches the line index in the document
    let range = lsp_types::Range::new(Position::new(1, 0), Position::new(1, 1));
    let (_, format_range, _) = documents.get_content_with_range(&uri, range).unwrap();
    assert_eq!(format_range, Some(2..3));
    change(&mut documents, &uri, vec![ranged((1, 0), (1, 1), "bbbb"), ranged((0, 0), (2, 1), "X")]);
    assert_eq!(documents.get_content(&uri).unwrap().0.as_str(), "X\n");
    // the line index is for the new text
    let range = lsp_types::Range::new(Position::new(0, 1), Position::new(1, 0));
    let (text, format_range, _) = documents.get_content_with_range(&uri, range).unwrap();
    assert_eq!(text.as_str(), "X\n");
    assert_eq!(format_range, Some(1..2));
  }

  #[test]
  fn changes_with_increasing_versions() {
    let (mut documents, uri) = open_document("a\n");
    change_with_version(&mut documents, &uri, 1, vec![ranged((0, 0), (0, 1), "b")]);
    change_with_version(&mut documents, &uri, 2, vec![ranged((0, 0), (0, 1), "c")]);
    // the version doesn't need to increase by one
    change_with_version(&mut documents, &uri, 5, vec![ranged((0, 0), (0, 1), "d")]);
    change_with_version(&mut documents, &uri, 6, vec![full("e\n")]);
    assert_eq!(documents.get_content(&uri).unwrap().0.as_str(), "e\n");
  }

  #[test]
  fn change_with_version_before_previous_change_forgets_document() {
    let (mut documents, uri) = open_document("a\n");
    change_with_version(&mut documents, &uri, 5, vec![ranged((0, 0), (0, 1), "b")]);
    // a late change that was made to text older than what's stored, but
    // which is still newer than the version the document was opened at
    change_with_version(&mut documents, &uri, 3, vec![ranged((0, 0), (0, 1), "c")]);
    assert_eq!(
      documents.environment.take_stderr_messages(),
      vec!["Changed version (3) was less than existing version (5) for 'file:///file.txt'. Forgetting document.".to_string()]
    );
    assert!(!documents.docs.contains_key(&uri));
  }

  #[test]
  fn change_with_version_before_opened_version_forgets_document() {
    let (mut documents, uri) = open_document_with_version("a\n", 4);
    change_with_version(&mut documents, &uri, 3, vec![ranged((0, 0), (0, 1), "b")]);
    assert_eq!(
      documents.environment.take_stderr_messages(),
      vec!["Changed version (3) was less than existing version (4) for 'file:///file.txt'. Forgetting document.".to_string()]
    );
    assert!(!documents.docs.contains_key(&uri));
  }

  #[test]
  fn change_with_same_version_is_applied() {
    // the same version as when opened
    let (mut documents, uri) = open_document_with_version("a\n", 2);
    change_with_version(&mut documents, &uri, 2, vec![ranged((0, 0), (0, 1), "b")]);
    assert_eq!(documents.get_content(&uri).unwrap().0.as_str(), "b\n");
    // the same version as the previous change
    change_with_version(&mut documents, &uri, 3, vec![ranged((0, 0), (0, 1), "c")]);
    change_with_version(&mut documents, &uri, 3, vec![ranged((0, 0), (0, 1), "d")]);
    assert_eq!(documents.get_content(&uri).unwrap().0.as_str(), "d\n");
    // the version is still checked after that
    change_with_version(&mut documents, &uri, 2, vec![ranged((0, 0), (0, 1), "e")]);
    assert_eq!(
      documents.environment.take_stderr_messages(),
      vec!["Changed version (2) was less than existing version (3) for 'file:///file.txt'. Forgetting document.".to_string()]
    );
    assert!(!documents.docs.contains_key(&uri));
  }

  fn apply_changes(text: &str, changes: Vec<TextDocumentContentChangeEvent>) -> String {
    let (mut documents, uri) = open_document(text);
    change(&mut documents, &uri, changes);
    documents.get_content(&uri).unwrap().0.to_string()
  }

  fn open_document(text: &str) -> (Documents<TestEnvironment>, Uri) {
    open_document_with_version(text, 0)
  }

  fn open_document_with_version(text: &str, version: i32) -> (Documents<TestEnvironment>, Uri) {
    let uri = Uri::from_str("file:///file.txt").unwrap();
    let mut documents = Documents::new(TestEnvironment::new());
    documents.open(TextDocumentItem {
      uri: uri.clone(),
      language_id: "txt".to_string(),
      version,
      text: text.to_string(),
    });
    (documents, uri)
  }

  fn change(documents: &mut Documents<TestEnvironment>, uri: &Uri, content_changes: Vec<TextDocumentContentChangeEvent>) {
    change_with_version(documents, uri, 1, content_changes);
  }

  fn change_with_version(documents: &mut Documents<TestEnvironment>, uri: &Uri, version: i32, content_changes: Vec<TextDocumentContentChangeEvent>) {
    documents.changed(DidChangeTextDocumentParams {
      text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version },
      content_changes,
    });
  }

  fn ranged(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
      range: Some(lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))),
      range_length: None,
      text: text.to_string(),
    }
  }

  fn full(text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
      range: None,
      range_length: None,
      text: text.to_string(),
    }
  }
}
