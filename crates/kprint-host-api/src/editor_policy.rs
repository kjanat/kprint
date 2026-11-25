use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use kprint_async_runtime::async_trait;
use url::Url;

/// An operation whose effects require a user's consent in an editor session.
pub enum EditorOperation<'a> {
  Download(&'a Url),
  ProcessPlugin {
    config: &'a ProcessPluginConfiguration,
    plugin: &'a str,
    artifact: &'a str,
  },
}

pub struct ProcessPluginConfiguration {
  pub source: String,
  pub fingerprint: String,
}

#[async_trait(?Send)]
pub trait EditorAuthorizer: Send + Sync {
  async fn authorize(&self, operation: EditorOperation<'_>) -> Result<()>;
}

/// An editor policy rejection should propagate rather than trigger a cache
/// retry or be treated as an ordinary connection failure.
#[derive(Debug)]
pub struct EditorApprovalRequired(pub String);

impl std::fmt::Display for EditorApprovalRequired {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str(&self.0)
  }
}

impl std::error::Error for EditorApprovalRequired {}

#[derive(Clone)]
pub struct EditorPolicy {
  authorizer: Arc<dyn EditorAuthorizer>,
  config: Option<Arc<ProcessPluginConfiguration>>,
}

tokio::task_local! {
  static EDITOR_POLICY: EditorPolicy;
}

impl EditorPolicy {
  pub fn new(authorizer: Arc<dyn EditorAuthorizer>) -> Self {
    Self { authorizer, config: None }
  }

  pub fn current() -> Option<Self> {
    EDITOR_POLICY.try_with(Clone::clone).ok()
  }

  pub async fn scope<T>(self, future: impl Future<Output = T>) -> T {
    EDITOR_POLICY.scope(self, future).await
  }

  pub async fn with_config<T>(mut self, config: ProcessPluginConfiguration, future: impl Future<Output = T>) -> T {
    self.config = Some(Arc::new(config));
    self.scope(future).await
  }
}

/// Inherit the policy when an editor operation creates another task. Regular
/// CLI and host operations have no editor policy.
pub async fn within_editor_policy<T>(policy: Option<EditorPolicy>, future: impl Future<Output = T>) -> T {
  match policy {
    Some(policy) => policy.scope(future).await,
    None => future.await,
  }
}

pub async fn authorize_download(url: &Url) -> Result<()> {
  if let Some(policy) = EditorPolicy::current() {
    policy.authorizer.authorize(EditorOperation::Download(url)).await?;
  }
  Ok(())
}

pub async fn authorize_process_plugin(plugin: &str, artifact: &str) -> Result<()> {
  if let Some(policy) = EditorPolicy::current() {
    let config = policy
      .config
      .as_deref()
      .ok_or_else(|| EditorApprovalRequired("Couldn't identify the editor's process-plugin configuration.".to_string()))?;
    policy.authorizer.authorize(EditorOperation::ProcessPlugin { config, plugin, artifact }).await?;
  }
  Ok(())
}
