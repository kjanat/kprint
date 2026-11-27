use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use futures::StreamExt;
use kprint_async_runtime::async_trait;
use kprint_configuration::ConfigKeyMap;
use kprint_configuration::GlobalConfiguration;
use kprint_markdown::EmbeddedCode;
use kprint_markdown::configuration::Configuration;
use kprint_plugin_types::FileMatchingInfo;
use kprint_plugin_types::FormatError;
use kprint_plugin_types::FormatRequest;
use kprint_plugin_types::FormatResult;
use kprint_plugin_types::HostFormatCallback;
use kprint_plugin_types::HostFormatRequest;
use kprint_plugin_types::HostFormatTarget;
use kprint_plugin_types::PluginInfo;
use kprint_plugin_types::PluginResolveConfigurationResult;

use super::in_process::InProcessHandler;
use super::in_process::InProcessPlugin;
use crate::environment::CanonicalizedPathBuf;
use crate::environment::PluginEnvironment;
use crate::plugins::BuiltInFormatter;
use crate::plugins::Plugin;
use crate::plugins::PluginSourceReference;
use crate::utils::PathSource;

const VERSION: &str = "0.29.0";
const PLUGIN_NAME: &str = "dprint-plugin-markdown";
const WASM_SHA256: &str = "714aea7555dd9831861be466da8b26b2d92316d3cde550ef3e1f63b31f4ea302";
const NPM_SHA256: &str = "d67ef8fb344b3e50ccc12c246f577303cad49eb0f7a7210731d59684504d72db";
const SIMULTANEOUS_BLOCKS: usize = 8;

static BUILT_IN: BuiltInFormatter = BuiltInFormatter {
  name: "markdown",
  cache_revision: 1,
  serves_plugin: PLUGIN_NAME,
  refers_to_served_plugin: serves_reference,
};

pub(crate) fn create_builtin_markdown_plugin<E: PluginEnvironment>(environment: &E, reference: &PluginSourceReference) -> Option<Box<dyn Plugin>> {
  if !serves_reference(reference) {
    return None;
  }
  let environment = environment.clone();
  Some(Box::new(InProcessPlugin::new(
    move || MarkdownHandler(environment.clone()),
    config_schema(),
    &BUILT_IN,
  )))
}

fn serves_reference(reference: &PluginSourceReference) -> bool {
  let checksum = reference.checksum.as_deref();
  match &reference.path_source {
    PathSource::Npm(npm) => {
      npm.specifier.name == "@dprint/markdown"
        && npm.specifier.version.as_deref() == Some(VERSION)
        && npm.specifier.path == "plugin.wasm"
        && checksum.is_none_or(|checksum| checksum == NPM_SHA256)
    }
    PathSource::Remote(remote) => {
      let url = &remote.url;
      url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && checksum.is_none_or(|checksum| checksum == WASM_SHA256)
        && matches!(
          (url.host_str(), url.path()),
          (
            Some("plugins.dprint.dev"),
            "/markdown-0.29.0.wasm" | "/dprint/dprint-plugin-markdown/0.29.0/plugin.wasm"
          ) | (Some("github.com"), "/dprint/dprint-plugin-markdown/releases/download/0.29.0/plugin.wasm")
            | (Some("cdn.jsdelivr.net"), "/npm/@dprint/markdown@0.29.0/plugin.wasm")
            | (Some("unpkg.com"), "/@dprint/markdown@0.29.0/plugin.wasm")
        )
    }
    PathSource::Local(_) => false,
  }
}

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "Schema for the user-facing Markdown configuration")]
struct MarkdownConfigSchema {
  #[serde(flatten)]
  settings: Configuration,
  deno: Option<bool>,
  #[serde(rename = "headingKind")]
  heading_kind: Option<kprint_markdown::configuration::HeadingKind>,
  #[serde(rename = "unorderedListKind")]
  unordered_list_kind: Option<kprint_markdown::configuration::ListUnorderedMarker>,
  #[serde(rename = "listIndentKind")]
  list_indent_kind: Option<kprint_markdown::configuration::ListIndentKind>,
}

fn config_schema() -> &'static str {
  static SCHEMA: OnceLock<String> = OnceLock::new();
  SCHEMA.get_or_init(|| serde_json::to_string(&schemars::schema_for!(MarkdownConfigSchema)).unwrap())
}

struct MarkdownHandler<E>(E);

#[async_trait(?Send)]
impl<E: PluginEnvironment> InProcessHandler for MarkdownHandler<E> {
  type Configuration = Configuration;
  const ACCEPTS_UNTRUSTED_CONFIGURATION: bool = true;

  fn plugin_info(&self) -> PluginInfo {
    PluginInfo {
      name: PLUGIN_NAME.to_string(),
      version: VERSION.to_string(),
      config_key: "markdown".to_string(),
      help_url: "https://dprint.dev/plugins/markdown".to_string(),
      config_schema_url: String::new(),
      update_url: Some("https://plugins.dprint.dev/dprint/dprint-plugin-markdown/latest.json".to_string()),
      supports_range_formatting: Some(true),
      allow_plugin_override_config: None,
      config_files: None,
      config_schema_paths: Vec::new(),
      config_local_schemas_use_root: false,
      config_reference_paths: Vec::new(),
      config_relative_path_bases: Default::default(),
    }
  }

  fn license_text(&self) -> String {
    kprint_markdown::LICENSE.to_string()
  }

  async fn resolve_config(
    &self,
    config: ConfigKeyMap,
    global: GlobalConfiguration,
    _base_dir: &CanonicalizedPathBuf,
  ) -> PluginResolveConfigurationResult<Configuration> {
    let resolved = kprint_markdown::configuration::resolve_config(config, &global);
    PluginResolveConfigurationResult {
      config: resolved.config,
      diagnostics: resolved.diagnostics,
      file_matching: FileMatchingInfo {
        file_extensions: ["md", "mkd", "mdwn", "mkdn", "mdown", "markdown"].into_iter().map(ToOwned::to_owned).collect(),
        file_names: Vec::new(),
        additive: false,
        additive_associations: Vec::new(),
      },
    }
  }

  async fn format(&self, request: FormatRequest<Configuration>, real_file_path: PathBuf, host: HostFormatCallback) -> FormatResult {
    let source = String::from_utf8(request.file_bytes)?;
    let mut resolved = BTreeMap::<BlockPosition, Result<Option<String>, FormatError>>::new();
    loop {
      if request.token.is_cancelled() {
        return Ok(None);
      }
      let mut pending = BTreeMap::new();
      let mut errors = BTreeMap::new();
      let mut embedded = |block: EmbeddedCode<'_>| {
        let key = BlockPosition {
          offset: block.offset,
          line_width: block.line_width,
        };
        match resolved.get(&key) {
          Some(Ok(text)) => text.clone(),
          Some(Err(error)) => {
            errors.insert(key, error);
            None
          }
          None => {
            pending
              .entry(key)
              .or_insert_with(|| (block.language.to_string(), block.text.as_bytes().to_vec()));
            None
          }
        }
      };
      let formatted = match &request.range {
        Some(range) => kprint_markdown::format_text_range(&source, range.clone(), &request.config, &mut embedded),
        None => kprint_markdown::format_text(&source, &request.config, &mut embedded),
      }
      .map_err(FormatError::new)?;

      if pending.is_empty() {
        for (key, error) in errors {
          let line = source_lines_before(&source, key.offset) + 1;
          let error = format!("{}:{line}: Could not format embedded content: {error}", real_file_path.display());
          if request.config.code_block_raise_syntax_errors {
            return Err(FormatError::new(error));
          }
          log_warn!(self.0, &error);
        }
        return Ok(formatted.map(String::into_bytes));
      }

      // Range formatting may widen when a replacement changes the surrounding
      // Markdown structure. Keep outcomes by source position across replays.
      let block_results = futures::stream::iter(pending)
        .map(|(key, (language, bytes))| {
          let target = match request.config.tags.get(&language.to_lowercase()) {
            Some(extension) => HostFormatTarget::Extension(extension.clone()),
            None => HostFormatTarget::Language(language),
          };
          let mut override_config = ConfigKeyMap::new();
          override_config.insert(
            "lineWidth".to_string(),
            kprint_configuration::ConfigKeyValue::Number(i32::try_from(key.line_width).unwrap_or(i32::MAX)),
          );
          if let Some(use_tabs) = request.config.code_block_use_tabs {
            override_config.insert("useTabs".to_string(), use_tabs.into());
          }
          if let Some(indent_width) = request.config.code_block_indent_width {
            override_config.insert("indentWidth".to_string(), i32::from(indent_width).into());
          }
          let format = host(HostFormatRequest {
            target,
            override_config,
            file_bytes: bytes,
            range: None,
            token: request.token.clone(),
          });
          async move {
            let result = format.await.and_then(|text| text.map(String::from_utf8).transpose().map_err(FormatError::from));
            (key, result)
          }
        })
        .buffer_unordered(SIMULTANEOUS_BLOCKS)
        .collect::<Vec<_>>();
      tokio::select! {
        _ = request.token.wait_cancellation() => return Ok(None),
        results = block_results => resolved.extend(results),
      }
    }
  }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BlockPosition {
  offset: usize,
  line_width: u32,
}

fn source_lines_before(source: &str, offset: usize) -> usize {
  source.as_bytes()[..offset]
    .iter()
    .enumerate()
    .filter(|&(index, byte)| *byte == b'\n' || (*byte == b'\r' && source.as_bytes().get(index + 1) != Some(&b'\n')))
    .count()
}

#[cfg(test)]
mod tests {
  use std::rc::Rc;
  use std::sync::Arc;
  use std::time::Duration;

  use kprint_plugin_types::FormatConfigId;
  use kprint_plugin_types::NullCancellationToken;

  use super::*;
  use crate::environment::TestEnvironment;

  #[test]
  fn substitutes_only_the_known_release() {
    let environment = TestEnvironment::new();
    let base = PathSource::new_local(CanonicalizedPathBuf::new_for_testing("/"));
    let serves = |source: &str| serves_reference(&crate::plugins::parse_plugin_source_reference(source, &base, &environment).unwrap());
    assert!(serves("npm:@dprint/markdown@0.29.0"));
    assert!(serves(&format!("npm:@dprint/markdown@0.29.0@{NPM_SHA256}")));
    assert!(!serves("npm:@dprint/markdown@0.24.0"));
    assert!(!serves(&format!("npm:@dprint/markdown@0.29.0@{WASM_SHA256}")));
    assert!(serves(&format!("https://plugins.dprint.dev/markdown-0.29.0.wasm@{WASM_SHA256}")));
    assert!(!serves("https://plugins.dprint.dev/markdown-0.29.0.wasm@bad"));
    assert!(!serves("https://example.com/markdown-0.29.0.wasm"));
  }

  #[test]
  fn generates_the_config_schema_from_the_options() {
    let schema: serde_json::Value = serde_json::from_str(config_schema()).unwrap();
    let config = serde_json::json!({
      "deno": false,
      "headingKind": "setext",
      "lineWidth": 100,
      "newLineKind": "lf",
      "codeBlock.raiseSyntaxErrors": true,
      "tags": { "example": "rs" }
    });
    assert_eq!(crate::test_helpers::validate_with_schema(&schema, &config), Ok(()));
    assert_eq!(crate::test_helpers::validate_with_schema(&schema, &serde_json::json!({})), Ok(()));
    assert!(crate::test_helpers::validate_with_schema(&schema, &serde_json::json!({"html.indentWidth": 300})).is_err());
    assert!(crate::test_helpers::validate_with_schema(&schema, &serde_json::json!({"heading.blankLinesAbove": 300})).is_err());
  }

  fn request(source: &str, config: Configuration) -> FormatRequest<Configuration> {
    FormatRequest {
      file_path: PathBuf::from("/guide.md"),
      file_bytes: source.as_bytes().to_vec(),
      config_id: FormatConfigId::from_raw(1),
      config: Arc::new(config),
      range: None,
      token: Arc::new(NullCancellationToken),
    }
  }

  #[test]
  fn formats_fences_concurrently_and_inserts_results_in_source_order() {
    let environment = TestEnvironment::new();
    environment.clone().run_in_runtime(async move {
      let rendezvous = Rc::new(tokio::sync::Barrier::new(2));
      let host: HostFormatCallback = Rc::new(move |request| {
        let rendezvous = rendezvous.clone();
        Box::pin(async move {
          assert_eq!(request.target, HostFormatTarget::Language("csharp".into()));
          rendezvous.wait().await;
          let text = String::from_utf8(request.file_bytes)?;
          Ok(Some(text.to_uppercase().into_bytes()))
        })
      });
      let request = request("```csharp\nfirst\n```\n\n```csharp\nsecond\n```\n", Configuration::default());
      let output = tokio::time::timeout(
        Duration::from_secs(1),
        MarkdownHandler(environment).format(request, PathBuf::from("/guide.md"), host),
      )
      .await
      .unwrap()
      .unwrap()
      .unwrap();
      assert_eq!(output, b"```csharp\nFIRST\n```\n\n```csharp\nSECOND\n```\n");
    });
  }

  #[test]
  fn reports_embed_errors_with_original_line_numbers() {
    let environment = TestEnvironment::new();
    environment.clone().run_in_runtime(async move {
      let host: HostFormatCallback = Rc::new(|_| Box::pin(async { Err(FormatError::new("formatter failed")) }));
      let source = "\n\u{FEFF}#  Title\n\n```csharp\nnot code\n```\n";
      let handler = MarkdownHandler(environment.clone());
      let path = PathBuf::from("/guide.md");
      let output = handler
        .format(request(source, Configuration::default()), path.clone(), host.clone())
        .await
        .unwrap()
        .unwrap();
      assert_eq!(output, b"# Title\n\n```csharp\nnot code\n```\n");
      let expected = "/guide.md:4: Could not format embedded content: formatter failed";
      assert_eq!(environment.take_stderr_messages(), [expected]);
      let config = Configuration {
        code_block_raise_syntax_errors: true,
        ..Default::default()
      };
      let error = handler.format(request(source, config), path, host).await.unwrap_err();
      assert_eq!(error.to_string(), expected);
      assert!(environment.take_stderr_messages().is_empty());
    });
  }
}
