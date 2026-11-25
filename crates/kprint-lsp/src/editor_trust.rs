use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use kprint_async_runtime::async_trait;
use kprint_config::resolution::resolve_global_config_dir_path;
use kprint_host_api::editor_policy::EditorApprovalRequired;
use kprint_host_api::editor_policy::EditorAuthorizer;
use kprint_host_api::editor_policy::EditorOperation;
use kprint_host_api::editor_policy::EditorPolicy;
use kprint_platform::environment::HostEnvironment;
use kprint_platform::utils::get_sha256_checksum;
use parking_lot::Mutex;
use serde_json::Value;
use url::Url;

const DEFAULT_TRUSTED_DOMAINS: [&str; 4] = ["registry.npmjs.org", "plugins.dprint.dev", "github.com", "githubusercontent.com"];

pub(crate) struct EditorTrust {
  trusted_domains: Vec<String>,
  allow_process_plugins: bool,
  decisions: Mutex<HashMap<String, Arc<tokio::sync::OnceCell<bool>>>>,
}

impl EditorTrust {
  pub(crate) fn new(environment: &impl HostEnvironment) -> Self {
    let mut trusted_domains = DEFAULT_TRUSTED_DOMAINS.iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut allow_process_plugins = false;
    if let Some(dir) = resolve_global_config_dir_path(environment) {
      let settings = dir.join("settings.json");
      let read_result = environment.maybe_read_file(&settings).map_err(anyhow::Error::from).and_then(|text| {
        text
          .map(|text| jsonc_parser::parse_to_serde_value::<Value>(&text, &Default::default()))
          .transpose()
          .map_err(anyhow::Error::from)
      });
      match read_result {
        Ok(Some(value)) => {
          allow_process_plugins = value.pointer("/editor/allowProcessPlugins").and_then(Value::as_bool) == Some(true);
          if let Some(domains) = value.pointer("/editor/trustedDomains").and_then(Value::as_array) {
            trusted_domains.extend(domains.iter().filter_map(Value::as_str).filter_map(normalize_domain));
          }
        }
        Ok(None) => {}
        Err(err) => {
          trusted_domains.clear();
          log_warn!(
            environment,
            "Could not read {}. Editor operations require approval. {:#}",
            settings.display(),
            err
          );
        }
      }
    }
    Self {
      trusted_domains,
      allow_process_plugins,
      decisions: Default::default(),
    }
  }

  pub(crate) fn approval(&self, operation: EditorOperation<'_>) -> Option<Approval> {
    match operation {
      EditorOperation::Download(url) => {
        if url.scheme() == "https"
          && url.host_str().is_some_and(|host| {
            let host = host.trim_end_matches('.');
            self.trusted_domains.iter().any(|domain| {
              host == domain
                || host
                  .strip_suffix(domain)
                  .is_some_and(|prefix| prefix.ends_with('.') && domain.parse::<std::net::IpAddr>().is_err())
            })
          })
        {
          return None;
        }
        let mut visible_url = url.clone();
        let _ = visible_url.set_username("");
        let _ = visible_url.set_password(None);
        visible_url.set_query(None);
        visible_url.set_fragment(None);
        Some(Approval {
          id: get_sha256_checksum(format!("download-v1:{}", url.as_str()).as_bytes()),
          message: format!("kprint needs permission to download {}.", prompt_text(visible_url.as_str())),
        })
      }
      EditorOperation::ProcessPlugin { config, plugin, artifact } => {
        if self.allow_process_plugins {
          return None;
        }
        Some(Approval {
          id: get_sha256_checksum(format!("process-v1:{}:{artifact}", config.fingerprint).as_bytes()),
          message: format!(
            "Formatting with {} requires running the unsandboxed process plugin {}.",
            prompt_text(&config.source),
            prompt_text(plugin)
          ),
        })
      }
    }
  }

  pub(crate) async fn decide(&self, approval: Approval, client: &crate::lsp::client::ClientWrapper) -> Result<()> {
    let decision = self.decisions.lock().entry(approval.id.clone()).or_default().clone();
    let allowed = decision.get_or_init(|| client.approve(approval.message.clone())).await;
    if *allowed { Ok(()) } else { Err(approval.required().into()) }
  }
}

fn normalize_domain(value: &str) -> Option<String> {
  let value = value.trim().trim_end_matches('.');
  let url = Url::parse(&format!("https://{value}/")).ok()?;
  if url.path() == "/" && url.username().is_empty() && url.password().is_none() && url.port().is_none() && url.query().is_none() && url.fragment().is_none() {
    url.host_str().map(str::to_string)
  } else {
    None
  }
}

fn prompt_text(value: &str) -> String {
  let mut escaped = String::from("\"");
  for character in value.chars() {
    if character.is_ascii() && !character.is_ascii_control() && !matches!(character, '"' | '\\' | '`') {
      escaped.push(character);
    } else {
      escaped.extend(character.escape_unicode());
    }
  }
  escaped.push('"');
  escaped
}

pub(crate) struct Approval {
  id: String,
  pub(crate) message: String,
}

impl Approval {
  fn required(self) -> EditorApprovalRequired {
    EditorApprovalRequired(self.message)
  }
}

#[async_trait(?Send)]
impl EditorAuthorizer for EditorTrust {
  async fn authorize(&self, operation: EditorOperation<'_>) -> Result<()> {
    match self.approval(operation) {
      Some(approval) => Err(
        EditorApprovalRequired(format!(
          "{} Set editor.trustedDomains or editor.allowProcessPlugins in the user's settings.json for editors without approval prompts.",
          approval.message
        ))
        .into(),
      ),
      None => Ok(()),
    }
  }
}

/// Policy for the schema 5 editor service and editor-info, which cannot request
/// approval over their existing transport.
pub fn legacy_editor_policy(environment: &impl HostEnvironment) -> EditorPolicy {
  EditorPolicy::new(Arc::new(EditorTrust::new(environment)))
}

pub(crate) fn lsp_editor_policy(environment: &impl HostEnvironment, client: crate::lsp::client::ClientWrapper) -> EditorPolicy {
  EditorPolicy::new(Arc::new(LspAuthorizer {
    trust: EditorTrust::new(environment),
    client,
  }))
}

struct LspAuthorizer {
  trust: EditorTrust,
  client: crate::lsp::client::ClientWrapper,
}

#[async_trait(?Send)]
impl EditorAuthorizer for LspAuthorizer {
  async fn authorize(&self, operation: EditorOperation<'_>) -> Result<()> {
    match self.trust.approval(operation) {
      Some(approval) => self.trust.decide(approval, &self.client).await,
      None => Ok(()),
    }
  }
}
