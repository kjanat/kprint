use std::sync::Arc;

use tower_lsp::Client;
use tower_lsp::lsp_types::MessageActionItem;
use tower_lsp::lsp_types::MessageType;
use tower_lsp::lsp_types::Registration;

#[kprint_async_runtime::async_trait(?Send)]
pub trait ClientTrait: std::fmt::Debug + Send + Sync {
  fn log(&self, message_type: MessageType, message: String);
  fn register(&self, _registrations: Vec<Registration>) {}
  async fn approve(&self, _message: String) -> bool {
    false
  }
}

#[kprint_async_runtime::async_trait(?Send)]
impl ClientTrait for Client {
  fn log(&self, message_type: MessageType, message: String) {
    let client = self.clone();
    kprint_async_runtime::spawn(async move {
      client.log_message(message_type, &message).await;
    });
  }

  fn register(&self, registrations: Vec<Registration>) {
    let client = self.clone();
    kprint_async_runtime::spawn(async move {
      let _ = client.register_capability(registrations).await;
    });
  }

  async fn approve(&self, message: String) -> bool {
    let decision = self
      .show_message_request(
        MessageType::WARNING,
        message,
        Some(vec![
          MessageActionItem {
            title: "Allow".to_string(),
            properties: Default::default(),
          },
          MessageActionItem {
            title: "Deny".to_string(),
            properties: Default::default(),
          },
        ]),
      )
      .await;
    matches!(decision, Ok(Some(action)) if action.title == "Allow")
  }
}

#[derive(Debug, Clone)]
pub struct ClientWrapper(Arc<dyn ClientTrait>);

impl ClientWrapper {
  pub fn new(client: Arc<dyn ClientTrait>) -> Self {
    Self(client)
  }

  pub fn log_info(&self, message: String) {
    self.log(MessageType::INFO, message);
  }

  pub async fn approve(&self, message: String) -> bool {
    self.0.approve(message).await
  }

  pub fn register(&self, registrations: Vec<Registration>) {
    self.0.register(registrations);
  }

  fn log(&self, message_type: MessageType, message: String) {
    self.0.log(message_type, message)
  }
}
