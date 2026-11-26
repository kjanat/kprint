mod vendor;

use std::path::PathBuf;
use std::sync::Arc;

use kprint_async_runtime::async_trait;
use kprint_async_runtime::future;
use kprint_configuration::ConfigKeyMap;
use kprint_configuration::GlobalConfiguration;
use kprint_plugin_types::CancellationToken;
use kprint_plugin_types::CheckConfigUpdatesMessage;
use kprint_plugin_types::ConfigChange;
use kprint_plugin_types::ConfigChangeKind;
use kprint_plugin_types::EmbeddedLanguageFormatRequest;
use kprint_plugin_types::FileMatchingInfo;
use kprint_plugin_types::FormatError;
use kprint_plugin_types::FormatRequest;
use kprint_plugin_types::FormatResult;
use kprint_plugin_types::HostFormatRequest;
use kprint_plugin_types::PluginInfo;
use kprint_plugin_types::PluginResolveConfigurationResult;

use super::in_process::InProcessFormatCallbacks;
use super::in_process::InProcessHandler;
use super::in_process::InProcessPlugin;
use crate::environment::CanonicalizedPathBuf;
use crate::environment::PluginEnvironment;
use crate::plugins::BuiltInFormatter;
use crate::plugins::Plugin;
use crate::plugins::PluginSourceReference;
use crate::utils::PathSource;

const VERSION: &str = "0.24.0";
const NPM_PACKAGE: &str = "@dprint/markdown";
const WASM_URL: &str = "https://plugins.dprint.dev/markdown-0.24.0.wasm";
const WASM_SHA256: &str = "cf7e65674b7eb5d91f85152ae9020dc8b3e73bd7944efcbfb05df71081217764";
const NPM_SHA256: &str = "7a1c8dc47c9d37ed7bd8736b629250013e082c4288fa4fc071dde64d83369fcc";
const MAX_NESTED_MARKDOWN: usize = 32;
const CONFIG_SCHEMA: &str = include_str!("vendor/schema.json");
const BUILT_IN: BuiltInFormatter = BuiltInFormatter {
  name: "markdown",
  cache_revision: 1,
  serves_plugin: "dprint-plugin-markdown",
  refers_to_served_plugin: is_builtin_markdown_reference,
};

pub(in crate::plugins) fn is_builtin_markdown_reference(reference: &PluginSourceReference) -> bool {
  let expected_checksum = match &reference.path_source {
    PathSource::Remote(remote) if remote.url.as_str() == WASM_URL => WASM_SHA256,
    PathSource::Npm(npm) if npm.specifier.name == NPM_PACKAGE && npm.specifier.version.as_deref() == Some(VERSION) && npm.specifier.path == "plugin.wasm" => {
      NPM_SHA256
    }
    _ => return false,
  };
  reference.checksum.as_deref().is_none_or(|checksum| checksum == expected_checksum)
}

pub(in crate::plugins) fn create_builtin_markdown_plugin<TEnvironment: PluginEnvironment>(environment: TEnvironment) -> Box<dyn Plugin> {
  Box::new(InProcessPlugin::new(
    move || MarkdownHandler {
      environment: environment.clone(),
    },
    CONFIG_SCHEMA,
    &BUILT_IN,
  ))
}

struct MarkdownHandler<TEnvironment> {
  environment: TEnvironment,
}

#[async_trait(?Send)]
impl<TEnvironment: PluginEnvironment> InProcessHandler for MarkdownHandler<TEnvironment> {
  type Configuration = vendor::configuration::Configuration;

  fn plugin_info(&self) -> PluginInfo {
    PluginInfo {
      name: BUILT_IN.serves_plugin.to_string(),
      version: VERSION.to_string(),
      config_key: "markdown".to_string(),
      help_url: "https://dprint.dev/plugins/markdown".to_string(),
      config_schema_url: format!("https://plugins.dprint.dev/dprint/dprint-plugin-markdown/{VERSION}/schema.json"),
      update_url: Some("https://plugins.dprint.dev/dprint/dprint-plugin-markdown/latest.json".to_string()),
      config_files: None,
      config_schema_paths: Vec::new(),
      config_local_schemas_use_root: false,
      config_reference_paths: Vec::new(),
      supports_range_formatting: None,
      allow_plugin_override_config: None,
      config_relative_path_bases: Default::default(),
    }
  }

  fn license_text(&self) -> String {
    include_str!("vendor/LICENSE").to_string()
  }

  fn uses_sandboxed_config_policy() -> bool {
    true
  }

  fn check_config_updates(&self, message: CheckConfigUpdatesMessage) -> Vec<ConfigChange> {
    let mut changes = Vec::new();
    for rename in vendor::configuration::get_config_key_renames(&message.config) {
      if let Some(value) = rename.value {
        changes.push(ConfigChange {
          path: vec![rename.new_key.to_string().into()],
          kind: ConfigChangeKind::Add(value),
        });
      }
      changes.push(ConfigChange {
        path: vec![rename.old_key.to_string().into()],
        kind: ConfigChangeKind::Remove,
      });
    }
    changes
  }

  async fn resolve_config(
    &self,
    config: ConfigKeyMap,
    global_config: GlobalConfiguration,
    _base_dir: &CanonicalizedPathBuf,
  ) -> PluginResolveConfigurationResult<Self::Configuration> {
    let resolved = vendor::configuration::resolve_config(config, &global_config);
    PluginResolveConfigurationResult {
      config: resolved.config,
      diagnostics: resolved.diagnostics,
      file_matching: FileMatchingInfo {
        file_extensions: ["md", "mkd", "mdwn", "mkdn", "mdown", "markdown"].map(str::to_string).into(),
        file_names: Vec::new(),
        additive: false,
        additive_associations: Vec::new(),
      },
    }
  }

  async fn format(&self, request: FormatRequest<Self::Configuration>, real_file_path: PathBuf, callbacks: InProcessFormatCallbacks) -> FormatResult {
    let text = std::str::from_utf8(&request.file_bytes)?;
    let format = MarkdownFormat {
      config: &request.config,
      callbacks,
      token: request.token,
    };
    let formatted = format
      .document(text, 0, 0)
      .await
      .map_err(|error| FormatError::new(format!("{}: {error}", real_file_path.display())))?;
    if format.token.is_cancelled() {
      return Ok(None);
    }
    for (line, error) in formatted.errors {
      log_warn!(
        self.environment,
        "{}:{}: Could not format Markdown code block: {}",
        real_file_path.display(),
        line,
        kprint_plugin_types::error_to_string(&error)
      );
    }
    Ok(formatted.text.map(String::into_bytes))
  }
}

struct MarkdownFormat<'a> {
  config: &'a vendor::configuration::Configuration,
  callbacks: InProcessFormatCallbacks,
  token: Arc<dyn CancellationToken>,
}

#[derive(Default)]
struct MarkdownResult {
  text: Option<String>,
  errors: Vec<(u32, vendor::FormatError)>,
}

impl MarkdownFormat<'_> {
  async fn document(&self, text: &str, line_offset: u32, depth: usize) -> Result<MarkdownResult, vendor::FormatError> {
    if self.token.is_cancelled() {
      return Ok(MarkdownResult::default());
    }
    let Some(document) = vendor::PreparedMarkdown::parse(text, self.config)? else {
      return Ok(MarkdownResult::default());
    };
    let requests = document.collect()?;
    let outcomes = future::join_all(requests.iter().map(|request| self.format_block(request, line_offset, depth))).await;
    if self.token.is_cancelled() {
      return Ok(MarkdownResult::default());
    }

    let mut errors = Vec::new();
    let results = requests
      .iter()
      .zip(outcomes)
      .map(|(request, outcome)| match outcome {
        Ok(mut formatted) => {
          errors.append(&mut formatted.errors);
          Ok(formatted.text)
        }
        Err(error) if !self.config.code_block_raise_syntax_errors => {
          errors.push((line_offset + request.line, error));
          Ok(None)
        }
        Err(error) => Err(error),
      })
      .collect();
    let text = document.render(requests, results)?;
    Ok(MarkdownResult { text, errors })
  }

  async fn format_block(&self, block: &vendor::CodeBlockRequest, line_offset: u32, depth: usize) -> Result<MarkdownResult, vendor::FormatError> {
    if block.tag.eq_ignore_ascii_case("markdown") || block.tag.eq_ignore_ascii_case("md") {
      // Nested Markdown uses the same vendored formatter, so it bypasses the
      // host's delegation depth check.
      if depth == MAX_NESTED_MARKDOWN {
        return Err(vendor::FormatError::CodeBlock("nested Markdown formatting depth exceeded".into()));
      }
      return Box::pin(self.document(&block.text, line_offset + block.content_line - 1, depth + 1)).await;
    }
    let mut override_config = ConfigKeyMap::new();
    override_config.insert("lineWidth".into(), (block.line_width as i32).into());
    if let Some(use_tabs) = self.config.code_block_use_tabs {
      override_config.insert("useTabs".into(), use_tabs.into());
    }
    if let Some(indent_width) = self.config.code_block_indent_width {
      override_config.insert("indentWidth".into(), i32::from(indent_width).into());
    }

    let result = if let Some(extension) = self.config.tags.get(&block.tag.trim().to_lowercase()) {
      if extension.contains(['/', '\\']) {
        return Err(vendor::FormatError::CodeBlock(format!("Invalid Markdown tag extension '{extension}'").into()));
      }
      (self.callbacks.path)(HostFormatRequest {
        file_path: PathBuf::from(format!("file.{extension}")),
        file_bytes: block.text.as_bytes().to_vec(),
        range: None,
        override_config,
        token: self.token.clone(),
      })
      .await
    } else {
      (self.callbacks.language)(EmbeddedLanguageFormatRequest {
        language: block.tag.clone(),
        file_bytes: block.text.as_bytes().to_vec(),
        range: None,
        override_config,
        token: self.token.clone(),
      })
      .await
    };
    let text = result
      .map_err(|error| vendor::FormatError::CodeBlock(error.into()))?
      .map(String::from_utf8)
      .transpose()?;
    Ok(MarkdownResult { text, errors: Vec::new() })
  }
}

#[cfg(test)]
mod tests {
  use std::cell::RefCell;
  use std::rc::Rc;
  use std::time::Duration;

  use kprint_plugin_types::FormatConfigId;
  use kprint_plugin_types::NullCancellationToken;
  use tokio::sync::oneshot;

  use super::*;
  use crate::environment::TestEnvironment;
  use crate::plugins::PluginCache;
  use crate::plugins::PluginResolver;
  use crate::plugins::parse_plugin_source_reference;

  #[tokio::test]
  async fn canonical_references_are_built_in_and_other_pins_use_normal_resolution() {
    let environment = TestEnvironment::new();
    let base = PathSource::new_local(CanonicalizedPathBuf::new_for_testing("/"));
    let parse = |reference: &str| parse_plugin_source_reference(reference, &base, &environment).unwrap();
    let resolver = PluginResolver::new(environment.clone(), PluginCache::new(environment.clone()));
    let markdown = resolver.resolve_plugin(parse(WASM_URL)).await.unwrap();
    assert_eq!(markdown.built_in().unwrap().name, "markdown");
    assert_eq!(markdown.info().version, VERSION);
    for reference in [
      format!("{WASM_URL}@{WASM_SHA256}"),
      format!("npm:{NPM_PACKAGE}@{VERSION}"),
      format!("npm:{NPM_PACKAGE}@{VERSION}/plugin.wasm@{NPM_SHA256}"),
    ] {
      let resolved = resolver.resolve_plugin(parse(&reference)).await.unwrap();
      assert!(Rc::ptr_eq(&markdown, &resolved), "{reference}");
    }
    for reference in [
      format!("{WASM_URL}@{NPM_SHA256}"),
      format!("{WASM_URL}?query=true"),
      format!("npm:{NPM_PACKAGE}"),
      format!("npm:{NPM_PACKAGE}@{VERSION}/other.wasm"),
      format!("npm:{NPM_PACKAGE}@{VERSION}@{WASM_SHA256}"),
      format!("npm:{NPM_PACKAGE}@0.24.1"),
      "https://example.com/markdown-0.24.0.wasm".to_string(),
      "/markdown-0.24.0.wasm".to_string(),
    ] {
      assert!(!is_builtin_markdown_reference(&parse(&reference)), "{reference}");
    }

    environment.add_remote_file(WASM_URL, b"fixture downloads should be validated");
    let mismatch = resolver.resolve_plugin(parse(&format!("{WASM_URL}@{NPM_SHA256}"))).await.err().unwrap();
    assert!(format!("{mismatch:#}").contains("Invalid checksum specified in configuration file"));
    environment.add_remote_file(
      &crate::test_helpers::npm_tarball_url(NPM_PACKAGE, VERSION),
      b"fixture npm downloads should be validated",
    );
    let mismatch = resolver
      .resolve_plugin(parse(&format!("npm:{NPM_PACKAGE}@{VERSION}@{WASM_SHA256}")))
      .await
      .err()
      .unwrap();
    assert!(format!("{mismatch:#}").contains("Invalid checksum for npm package"));
    let alternate_url = "https://plugins.dprint.dev/markdown-0.24.1.wasm";
    environment.add_remote_file_error(alternate_url, "fixture normal Markdown download");
    let alternate = resolver.resolve_plugin(parse(alternate_url)).await.err().unwrap();
    assert!(format!("{alternate:#}").contains("fixture normal Markdown download"));
    environment.clear_logs();
  }

  #[tokio::test]
  async fn identical_blocks_format_concurrently_and_replay_in_their_source_order() {
    let config = vendor::configuration::ConfigurationBuilder::new().line_width(70).build();
    let token = Arc::new(NullCancellationToken);
    let entered = Rc::new(RefCell::new(Vec::new()));
    let (sibling_entered, wait_for_sibling) = oneshot::channel();
    let sibling_entered = RefCell::new(Some(sibling_entered));
    let wait_for_sibling = RefCell::new(Some(wait_for_sibling));
    let format = MarkdownFormat {
      config: &config,
      token,
      callbacks: InProcessFormatCallbacks {
        path: Rc::new(|_| panic!("raw labels use language formatting")),
        language: Rc::new({
          let entered = entered.clone();
          move |request| {
            assert_eq!(request.language, "csharp");
            assert_eq!(request.file_bytes, b"same body");
            assert!(request.range.is_none());
            let mut entered = entered.borrow_mut();
            entered.push(request.override_config["lineWidth"].as_number().unwrap());
            if entered.len() == 1 {
              let wait_for_sibling = wait_for_sibling.borrow_mut().take().unwrap();
              Box::pin(async move {
                wait_for_sibling.await.unwrap();
                Ok(Some(b"first result".to_vec()))
              })
            } else {
              sibling_entered.borrow_mut().take().unwrap().send(()).unwrap();
              Box::pin(async { Ok(Some(b"second result".to_vec())) })
            }
          }
        }),
      },
    };
    let text = "```csharp\nsame body\n```\n\n> - ```csharp\n>   same body\n>   ```\n";
    let result = tokio::time::timeout(Duration::from_secs(5), format.document(text, 0, 0))
      .await
      .expect("the first formatter waits for the second to start")
      .unwrap();
    let entered = entered.borrow();
    assert_eq!(entered[0], 70);
    assert!(entered[1] < entered[0], "{entered:?}");
    assert_eq!(
      result.text.unwrap(),
      "```csharp\nfirst result\n```\n\n> - ```csharp\n>   second result\n>   ```\n"
    );
  }

  #[tokio::test]
  async fn limits_nested_markdown_without_losing_the_unformatted_content() {
    let config = vendor::configuration::ConfigurationBuilder::new().build();
    let format = MarkdownFormat {
      config: &config,
      token: Arc::new(NullCancellationToken),
      callbacks: InProcessFormatCallbacks {
        path: Rc::new(|_| panic!("nested Markdown formats in process")),
        language: Rc::new(|_| panic!("nested Markdown formats in process")),
      },
    };
    let mut text = "#   unchanged\n".to_string();
    for ticks in 3..=3 + MAX_NESTED_MARKDOWN {
      let fence = "`".repeat(ticks);
      text = format!("{fence}md\n{text}{fence}\n");
    }
    let result = format.document(&text, 0, 0).await.unwrap();
    assert!(result.text.unwrap_or(text).contains("#   unchanged"));
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].0 as usize, MAX_NESTED_MARKDOWN + 1);
    assert!(result.errors[0].1.to_string().contains("nested Markdown formatting depth exceeded"));
  }

  #[tokio::test]
  async fn warns_at_each_source_fence_and_reports_strict_failures() {
    let environment = TestEnvironment::new();
    let handler = MarkdownHandler {
      environment: environment.clone(),
    };
    let source_path = PathBuf::from("docs").join("sample.md");
    let text = "\u{feff}#  Title\n\n```bad\nsame\n```\n\n> ````markdown\n> \n> ```bad\n> same\n> ```\n> ````\n\n```custom\ndata\n```\n";
    for strict in [false, true] {
      let mut config = vendor::configuration::ConfigurationBuilder::new()
        .code_block_raise_syntax_errors(strict)
        .build();
      config.tags.insert("custom".into(), "txt".into());
      let output = handler
        .format(
          FormatRequest {
            file_path: PathBuf::from("synthetic.md"),
            file_bytes: text.as_bytes().to_vec(),
            config_id: FormatConfigId::uninitialized(),
            config: Arc::new(config),
            range: None,
            token: Arc::new(NullCancellationToken),
          },
          source_path.clone(),
          InProcessFormatCallbacks {
            path: Rc::new(|request| {
              assert_eq!(request.file_path, PathBuf::from("file.txt"));
              assert_eq!(request.file_bytes, b"data");
              Box::pin(async { Ok(Some(b"formatted data".to_vec())) })
            }),
            language: Rc::new(|request| {
              assert_eq!(request.language, "bad");
              assert_eq!(request.file_bytes, b"same");
              Box::pin(async { Err(FormatError::from("test formatter failed")) })
            }),
          },
        )
        .await;
      let warnings = environment.take_stderr_messages();
      if strict {
        let error = output.unwrap_err().to_string();
        assert!(error.contains(&source_path.display().to_string()), "{error}");
        assert!(error.contains("line 3"), "{error}");
        assert!(error.contains("test formatter failed"), "{error}");
        assert!(warnings.is_empty(), "{warnings:?}");
      } else {
        let output = String::from_utf8(output.unwrap().unwrap()).unwrap();
        assert_eq!(output.matches("same").count(), 2);
        assert!(output.contains("formatted data"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        for (warning, line) in warnings.into_iter().zip([3, 9]) {
          assert!(warning.contains(&format!("{}:{line}:", source_path.display())), "{warning}");
          assert!(warning.contains("test formatter failed"), "{warning}");
        }
      }
    }
    environment.clear_logs();
  }
}
