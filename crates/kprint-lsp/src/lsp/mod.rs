use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use kprint_async_runtime::JoinHandle;
use kprint_host_api::editor_policy::EditorPolicy;
use kprint_plugin_types::FormatRange;
use kprint_plugin_types::HostFormatRequest;
use kprint_process_plugin::start_parent_process_checker_task;
use parking_lot::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::try_join;
use tokio_util::sync::CancellationToken;
use tower_lsp::LanguageServer;
use tower_lsp::LspService;
use tower_lsp::Server;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionOptions;
use tower_lsp::lsp_types::CompletionParams;
use tower_lsp::lsp_types::CompletionResponse;
use tower_lsp::lsp_types::DidChangeNotebookDocumentParams;
use tower_lsp::lsp_types::DidChangeTextDocumentParams;
use tower_lsp::lsp_types::DidChangeWorkspaceFoldersParams;
use tower_lsp::lsp_types::DidCloseNotebookDocumentParams;
use tower_lsp::lsp_types::DidCloseTextDocumentParams;
use tower_lsp::lsp_types::DidOpenNotebookDocumentParams;
use tower_lsp::lsp_types::DidOpenTextDocumentParams;
use tower_lsp::lsp_types::DocumentFormattingParams;
use tower_lsp::lsp_types::DocumentRangeFormattingParams;
use tower_lsp::lsp_types::Hover;
use tower_lsp::lsp_types::HoverParams;
use tower_lsp::lsp_types::HoverProviderCapability;
use tower_lsp::lsp_types::InitializeParams;
use tower_lsp::lsp_types::InitializeResult;
use tower_lsp::lsp_types::InitializedParams;
use tower_lsp::lsp_types::OneOf;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Registration;
use tower_lsp::lsp_types::ServerCapabilities;
use tower_lsp::lsp_types::ServerInfo;
use tower_lsp::lsp_types::TextDocumentSyncCapability;
use tower_lsp::lsp_types::TextDocumentSyncKind;
use tower_lsp::lsp_types::TextDocumentSyncOptions;
use tower_lsp::lsp_types::TextEdit;
use tower_lsp::lsp_types::Uri;
use tower_lsp::lsp_types::WorkspaceFoldersServerCapabilities;
use tower_lsp::lsp_types::WorkspaceServerCapabilities;
use url::Url;

use crate::configuration::config_needs_file_message;
use crate::configuration::is_stream_path;
use crate::configuration::resolve_uncanonicalized_local_path;
use crate::environment::HostEnvironment as Environment;
use crate::plugins::PluginResolver;
use kprint_config::options::ConfigOptions;
#[cfg(test)]
use kprint_host_api::options::SessionOptions;

use self::client::ClientWrapper;
use self::config_completion::ConfigCompletions;
use self::config_completion::is_config_uri;
use self::documents::Documents;
use self::text::LineIndex;
use self::text::RangeFormat;
use self::text::get_range_utf16_edits;
use self::text::normalize_to_source_line_endings;
use self::text::to_lsp_edits;
use kprint_host::HostSession;

pub(crate) mod client;
mod config_completion;
mod documents;
mod language;
mod notebook;
mod text;
mod untitled;

// tower-lsp will drop the future on cancellation,
// so use this to cancel the containing token on drop.
struct DropToken {
  completed: bool,
  token: Arc<CancellationToken>,
}

impl DropToken {
  pub fn new(token: Arc<CancellationToken>) -> Self {
    Self { token, completed: false }
  }

  pub fn completed(&mut self) {
    self.completed = true;
  }
}

impl Drop for DropToken {
  fn drop(&mut self) {
    if !self.completed {
      self.token.cancel();
    }
  }
}

struct PendingTokenGuard {
  id: u16,
  tokens: Rc<RefCell<HashMap<u16, Arc<CancellationToken>>>>,
}

impl Drop for PendingTokenGuard {
  fn drop(&mut self) {
    self.tokens.borrow_mut().remove(&self.id);
  }
}

#[derive(Default)]
struct PendingTokens {
  next_id: u16,
  tokens: Rc<RefCell<HashMap<u16, Arc<CancellationToken>>>>,
}

impl PendingTokens {
  pub fn insert(&mut self, token: Arc<CancellationToken>) -> PendingTokenGuard {
    let id = self.next_id();
    self.tokens.borrow_mut().insert(id, token);
    PendingTokenGuard {
      id,
      tokens: self.tokens.clone(),
    }
  }

  pub fn cancel_all(&mut self) {
    let mut pending_tokens = self.tokens.borrow_mut();
    for token in pending_tokens.values() {
      token.cancel();
    }
    pending_tokens.clear();
    self.next_id = 0;
  }

  pub fn next_id(&mut self) -> u16 {
    if self.next_id == u16::MAX {
      self.next_id = 0;
    }
    let id = self.next_id;
    self.next_id += 1;
    id
  }
}

struct EditorFormatRequest {
  pub file_path: PathBuf,
  pub notebook_path: Option<PathBuf>,
  pub file_text: String,
  pub maybe_line_index: Option<LineIndex>,
  pub range: FormatRange,
  pub token: Arc<CancellationToken>,
}

struct ConfigEditorRequest {
  pub file_path: PathBuf,
  pub file_text: String,
  pub position: Position,
}

enum ChannelMessage {
  Format(EditorFormatRequest, oneshot::Sender<Result<Option<Vec<TextEdit>>>>),
  Completion(ConfigEditorRequest, oneshot::Sender<Option<Vec<CompletionItem>>>),
  Hover(ConfigEditorRequest, oneshot::Sender<Option<Hover>>),
  Shutdown(oneshot::Sender<()>),
  /// This message is used for testing.
  #[cfg(test)]
  HasPending(oneshot::Sender<bool>),
}

async fn handle_format_request<TEnvironment: Environment>(
  mut request: EditorFormatRequest,
  scope_container: Rc<HostSession<TEnvironment>>,
  environment: &TEnvironment,
) -> Result<Option<Vec<TextEdit>>> {
  let Some(parent_dir) = request.file_path.parent() else {
    log_warn!(environment, "Cannot format non-file path: {}", request.file_path.display());
    return Ok(None);
  };
  if request.token.is_cancelled() {
    return Ok(None);
  }
  let Some(scope) = scope_container.resolve_by_path(parent_dir).await? else {
    log_stderr_info!(environment, "Path did not have a dprint config file: {}", request.file_path.display());
    return Ok(None);
  };
  if request.token.is_cancelled() {
    return Ok(None);
  }
  // canonicalize the path
  request.file_path = environment
    .canonicalize_maybe_not_exists(&request.file_path)
    .map(|p| p.into_path_buf())
    .unwrap_or(request.file_path);

  let can_format = match &request.notebook_path {
    Some(path) => {
      let canonical_path = environment.canonicalize_maybe_not_exists(path).ok();
      let path = canonical_path.as_ref().map(AsRef::as_ref).unwrap_or(path);
      !scope.plugin_name_maps.get_plugin_names_from_file_path(path).into_names().is_empty() && scope.can_format_for_editor(path, None)
    }
    None => scope.can_format_for_editor(&request.file_path, Some(request.file_text.as_bytes())),
  };
  if !can_format {
    log_debug!(environment, "Excluded file: {}", request.file_path.display());
    return Ok(None);
  }

  let supports_range = scope.supports_range_formatting(&request.file_path, request.file_text.as_bytes());
  if request.range.is_some() && supports_range == Some(false) {
    log_debug!(environment, "A plugin does not support formatting a range: {}", request.file_path.display());
    return Ok(None);
  }
  let notebook_range = request.notebook_path.as_ref().map(|_| request.range.clone());
  let range = RangeFormat::new(&request.file_text, request.range, supports_range == Some(true));
  let Some(result) = scope
    .format(HostFormatRequest {
      file_path: request.file_path,
      file_bytes: request.file_text.as_bytes().to_vec(),
      range: range.plugin_range,
      override_config: Default::default(),
      token: request.token,
    })
    .await?
  else {
    return Ok(None);
  };
  kprint_async_runtime::spawn_blocking(move || {
    let new_text = String::from_utf8(result).context("Failed converting formatted text to utf-8.")?;
    // the editor owns the document's line endings, so match them rather than
    // imposing the plugin's configured newline kind (see #965)
    let mut new_text = normalize_to_source_line_endings(&request.file_text, new_text);
    if let Some(notebook_range) = notebook_range {
      new_text = notebook::trim_formatted_cell_text(&request.file_text, new_text, &notebook_range);
    }
    let line_index = request.maybe_line_index.unwrap_or_else(|| LineIndex::new(&request.file_text));
    Ok(Some(to_lsp_edits(
      get_range_utf16_edits(&request.file_text, &new_text, &range.edits_range),
      &line_index,
    )))
  })
  .await?
}

/// The `--config` override for the language server, which needs a configuration
/// file it can re-read for as long as the server runs.
fn resolve_config_override(args: &dyn ConfigOptions, environment: &impl Environment) -> anyhow::Result<Option<PathBuf>> {
  let Some(config) = args.config().as_ref().and_then(|config| config.maybe_path_or_url()) else {
    return Ok(None);
  };
  let Some(path) = resolve_uncanonicalized_local_path(config, &environment.cwd(), environment) else {
    anyhow::bail!("kprint lsp --config requires a local configuration file. Extend a remote configuration from that file.");
  };
  // a pipe hands over its text once and has no file behind it to read again,
  // so turn it away at startup rather than failing every formatting request
  if is_stream_path(environment, &path) {
    anyhow::bail!("{}", config_needs_file_message(&path.to_string_lossy()));
  }
  Ok(Some(path))
}

pub async fn run_language_server<TEnvironment: Environment>(
  args: &dyn ConfigOptions,
  environment: &TEnvironment,
  plugin_resolver: &Rc<PluginResolver<TEnvironment>>,
) -> anyhow::Result<()> {
  let stdin = tokio::io::stdin();
  let stdout = tokio::io::stdout();
  let (tx, rx) = mpsc::unbounded_channel();

  let config_path = resolve_config_override(args, environment)?;
  let mut policy = None;
  let (service, socket, pending) = LspService::new(|client| {
    let client = ClientWrapper::new(Arc::new(client));
    policy = Some(crate::editor_trust::lsp_editor_policy(environment, client.clone()));
    Backend::new(client, environment.clone(), tx)
  });
  let recv_task = start_message_handler(environment, plugin_resolver, config_path, policy.unwrap(), rx);
  let lsp_task = kprint_async_runtime::spawn(Server::new(stdin, stdout, socket, pending).serve(service));

  try_join!(recv_task, lsp_task)?;

  Ok(())
}

fn start_message_handler<TEnvironment: Environment>(
  environment: &TEnvironment,
  plugin_resolver: &Rc<PluginResolver<TEnvironment>>,
  config_override: Option<PathBuf>,
  policy: EditorPolicy,
  mut rx: mpsc::UnboundedReceiver<ChannelMessage>,
) -> JoinHandle<()> {
  // tower_lsp requires Backend to implement Send and Sync, but
  // we use a single threaded runtime. So spawn some tasks and
  // communicate over a channel.
  let max_cores = environment.max_threads();
  let concurrency_limiter = Rc::new(Semaphore::new(std::cmp::max(1, max_cores - 1)));
  let environment = environment.clone();
  let scope_container = Rc::new(HostSession::new(environment.clone(), plugin_resolver.clone(), config_override));
  let config_completions = Rc::new(ConfigCompletions::new(environment.clone(), scope_container.clone()));
  kprint_async_runtime::spawn(async move {
    let mut pending_tokens = PendingTokens::default();
    while let Some(message) = rx.recv().await {
      match message {
        ChannelMessage::Format(request, sender) => {
          let token_guard = pending_tokens.insert(request.token.clone());
          let concurrency_limiter = concurrency_limiter.clone();
          let scope_container = scope_container.clone();
          let environment = environment.clone();
          kprint_async_runtime::spawn(policy.clone().scope(async move {
            let _permit = concurrency_limiter.acquire().await;
            let result = handle_format_request(request, scope_container, &environment).await;
            let _ = sender.send(result);
            drop(token_guard); // remove the token from the pending tokens
          }));
        }
        ChannelMessage::Completion(request, sender) => {
          let config_completions = config_completions.clone();
          kprint_async_runtime::spawn(policy.clone().scope(async move {
            let result = config_completions.completions(&request.file_path, &request.file_text, request.position).await;
            let _ = sender.send(result);
          }));
        }
        ChannelMessage::Hover(request, sender) => {
          let config_completions = config_completions.clone();
          kprint_async_runtime::spawn(policy.clone().scope(async move {
            let result = config_completions.hover(&request.file_path, &request.file_text, request.position).await;
            let _ = sender.send(result);
          }));
        }
        ChannelMessage::Shutdown(sender) => {
          pending_tokens.cancel_all();
          scope_container.shutdown().await;
          let _ = sender.send(());
          break; // exit
        }
        #[cfg(test)]
        ChannelMessage::HasPending(sender) => {
          let is_empty = pending_tokens.tokens.borrow().is_empty();
          let _ = sender.send(!is_empty);
        }
      }
    }
  })
}

struct State<TEnvironment: Environment> {
  documents: Documents<TEnvironment>,
  workspace_folders: Vec<PathBuf>,
  registrations: Vec<Registration>,
}

struct Backend<TEnvironment: Environment> {
  client: ClientWrapper,
  environment: TEnvironment,
  sender: mpsc::UnboundedSender<ChannelMessage>,
  state: Mutex<State<TEnvironment>>,
}

impl<TEnvironment: Environment> Backend<TEnvironment> {
  pub fn new(client: ClientWrapper, environment: TEnvironment, sender: mpsc::UnboundedSender<ChannelMessage>) -> Self {
    Backend {
      client,
      environment: environment.clone(),
      sender,
      state: Mutex::new(State {
        documents: Documents::new(environment),
        workspace_folders: Vec::new(),
        registrations: Vec::new(),
      }),
    }
  }

  fn format_paths(&self, uri: &Uri) -> Option<(PathBuf, Option<PathBuf>)> {
    let state = self.state.lock();
    if untitled::is_untitled_uri(uri) {
      let home = self.environment.get_home_dir();
      let folder = state.workspace_folders.first().map(PathBuf::as_path).or(home.as_ref().map(AsRef::as_ref))?;
      let path = untitled::get_untitled_file_path(folder, state.documents.language_id(uri)?)?;
      return Some((path, None));
    }
    if let Some((notebook_uri, language_id)) = state.documents.notebook_cell(uri) {
      let notebook_path = uri_to_file_path(notebook_uri)?;
      let cell_path = notebook::get_notebook_cell_file_path(&notebook_path, language_id)?;
      return Some((cell_path, Some(notebook_path)));
    }
    Some((uri_to_file_path(uri)?, None))
  }

  async fn send_format_request(&self, uri: &Uri, request: EditorFormatRequest) -> LspResult<Option<Vec<TextEdit>>> {
    let start_time = std::time::Instant::now();
    log_debug!(self.environment, "Received format request for {}", uri.as_str());
    let mut drop_token = DropToken::new(request.token.clone());
    let result = self.send_format_request_inner(request).await;
    drop_token.completed();
    let result = match result {
      Ok(value) => Ok(value),
      Err(err) => {
        log_error!(self.environment, "Failed formatting '{}': {:#}", uri.as_str(), err);
        Ok(None)
      }
    };
    log_debug!(
      self.environment,
      "Finished format request for {} in {}ms",
      uri.as_str(),
      start_time.elapsed().as_millis()
    );
    result
  }

  async fn send_format_request_inner(&self, request: EditorFormatRequest) -> Result<Option<Vec<TextEdit>>> {
    let (sender, receiver) = oneshot::channel();
    self.sender.send(ChannelMessage::Format(request, sender))?;
    receiver.await?
  }

  /// This is used in the test code to ensure there are no pending requests.
  #[cfg(test)]
  pub async fn has_pending(&self) -> bool {
    let (sender, receiver) = oneshot::channel();
    if self.sender.send(ChannelMessage::HasPending(sender)).is_ok() {
      receiver.await.unwrap_or(false)
    } else {
      false
    }
  }
}

#[tower_lsp::async_trait(?Send)]
impl<TEnvironment: Environment> LanguageServer for Backend<TEnvironment> {
  async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
    if let Some(parent_id) = params.process_id {
      start_parent_process_checker_task(parent_id);
    }

    {
      let mut state = self.state.lock();
      state.registrations = notebook::get_notebook_cell_format_registrations(&params.capabilities);
      state.registrations.extend(untitled::get_untitled_registrations(&params.capabilities));
      state.workspace_folders = match &params.workspace_folders {
        Some(folders) => folders.iter().filter_map(|folder| uri_to_file_path(&folder.uri)).collect(),
        #[allow(deprecated)]
        None => params.root_uri.as_ref().and_then(uri_to_file_path).into_iter().collect(),
      };
    }
    Ok(InitializeResult {
      server_info: Some(ServerInfo {
        name: "kprint".to_string(),
        version: Some(self.environment.cli_display_version()),
      }),
      capabilities: ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
          // todo: incremental should work now, but let's try out full to start
          change: Some(TextDocumentSyncKind::FULL),
          open_close: Some(true),
          save: None,
          will_save: None,
          will_save_wait_until: None,
        })),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
          // `"` opens a property/value string, `:` moves to a value position
          trigger_characters: Some(vec!["\"".to_string(), ":".to_string()]),
          ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        notebook_document_sync: params
          .capabilities
          .notebook_document
          .map(|_| OneOf::Left(notebook::get_notebook_document_sync_options())),
        workspace: Some(WorkspaceServerCapabilities {
          workspace_folders: Some(WorkspaceFoldersServerCapabilities {
            supported: Some(true),
            change_notifications: Some(OneOf::Left(true)),
          }),
          file_operations: None,
        }),
        ..ServerCapabilities::default()
      },
    })
  }

  async fn initialized(&self, _: InitializedParams) {
    self.client.log_info(format!(
      "kprint {} ({}-{})",
      self.environment.cli_display_version(),
      self.environment.os(),
      self.environment.cpu_arch()
    ));
    self.client.log_info("Server ready.".to_string());
    let registrations = std::mem::take(&mut self.state.lock().registrations);
    if !registrations.is_empty() {
      self.client.register(registrations);
    }
  }

  async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
    let removed = params
      .event
      .removed
      .iter()
      .filter_map(|folder| uri_to_file_path(&folder.uri))
      .collect::<Vec<_>>();
    let mut state = self.state.lock();
    state.workspace_folders.retain(|folder| !removed.contains(folder));
    for path in params.event.added.iter().filter_map(|folder| uri_to_file_path(&folder.uri)) {
      if !state.workspace_folders.contains(&path) {
        state.workspace_folders.push(path);
      }
    }
  }

  async fn did_open(&self, params: DidOpenTextDocumentParams) {
    self.state.lock().documents.open(params.text_document);
  }

  async fn did_change(&self, params: DidChangeTextDocumentParams) {
    self.state.lock().documents.changed(params);
  }

  async fn did_close(&self, params: DidCloseTextDocumentParams) {
    self.state.lock().documents.closed(&params.text_document.uri);
  }

  async fn notebook_did_open(&self, params: DidOpenNotebookDocumentParams) {
    let mut state = self.state.lock();
    let uri = &params.notebook_document.uri;
    state.documents.closed_notebook(uri);
    for document in params.cell_text_documents {
      state.documents.open_notebook_cell(uri, document);
    }
  }

  async fn notebook_did_change(&self, params: DidChangeNotebookDocumentParams) {
    let Some(cells) = params.change.cells else {
      return;
    };
    let mut state = self.state.lock();
    if let Some(structure) = cells.structure {
      for document in structure.did_close.unwrap_or_default() {
        state.documents.closed(&document.uri);
      }
      for document in structure.did_open.unwrap_or_default() {
        state.documents.open_notebook_cell(&params.notebook_document.uri, document);
      }
    }
    for change in cells.text_content.unwrap_or_default() {
      state.documents.changed(DidChangeTextDocumentParams {
        text_document: change.document,
        content_changes: change.changes,
      });
    }
  }

  async fn notebook_did_close(&self, params: DidCloseNotebookDocumentParams) {
    self.state.lock().documents.closed_notebook(&params.notebook_document.uri);
  }

  async fn formatting(&self, params: DocumentFormattingParams, token: CancellationToken) -> LspResult<Option<Vec<TextEdit>>> {
    let Some((file_path, notebook_path)) = self.format_paths(&params.text_document.uri) else {
      return Ok(None);
    };
    let Some((file_text, maybe_line_index)) = self.state.lock().documents.get_content(&params.text_document.uri) else {
      return Ok(None);
    };
    self
      .send_format_request(
        &params.text_document.uri,
        EditorFormatRequest {
          file_path,
          notebook_path,
          file_text,
          range: None,
          maybe_line_index,
          token: Arc::new(token),
        },
      )
      .await
  }

  async fn range_formatting(&self, params: DocumentRangeFormattingParams, token: CancellationToken) -> LspResult<Option<Vec<TextEdit>>> {
    let Some((file_path, notebook_path)) = self.format_paths(&params.text_document.uri) else {
      return Ok(None);
    };
    let Some((file_text, range, line_index)) = self.state.lock().documents.get_content_with_range(&params.text_document.uri, params.range) else {
      return Ok(None);
    };
    self
      .send_format_request(
        &params.text_document.uri,
        EditorFormatRequest {
          file_path,
          notebook_path,
          file_text,
          range,
          maybe_line_index: Some(line_index),
          token: Arc::new(token),
        },
      )
      .await
  }

  async fn completion(&self, params: CompletionParams, _token: CancellationToken) -> LspResult<Option<CompletionResponse>> {
    let uri = params.text_document_position.text_document.uri;
    if !is_config_uri(&uri) {
      return Ok(None);
    }
    let Some(file_path) = uri_to_file_path(&uri) else {
      return Ok(None);
    };
    let Some((file_text, _)) = self.state.lock().documents.get_content(&uri) else {
      return Ok(None);
    };
    let (sender, receiver) = oneshot::channel();
    let request = ConfigEditorRequest {
      file_path,
      file_text,
      position: params.text_document_position.position,
    };
    if self.sender.send(ChannelMessage::Completion(request, sender)).is_err() {
      return Ok(None);
    }
    Ok(receiver.await.ok().flatten().map(CompletionResponse::Array))
  }

  async fn hover(&self, params: HoverParams, _token: CancellationToken) -> LspResult<Option<Hover>> {
    let uri = params.text_document_position_params.text_document.uri;
    if !is_config_uri(&uri) {
      return Ok(None);
    }
    let Some(file_path) = uri_to_file_path(&uri) else {
      return Ok(None);
    };
    let Some((file_text, _)) = self.state.lock().documents.get_content(&uri) else {
      return Ok(None);
    };
    let (sender, receiver) = oneshot::channel();
    let request = ConfigEditorRequest {
      file_path,
      file_text,
      position: params.text_document_position_params.position,
    };
    if self.sender.send(ChannelMessage::Hover(request, sender)).is_err() {
      return Ok(None);
    }
    Ok(receiver.await.ok().flatten())
  }

  async fn shutdown(&self) -> LspResult<()> {
    let (sender, receiver) = oneshot::channel();
    if self.sender.send(ChannelMessage::Shutdown(sender)).is_ok() {
      let _ = receiver.await;
    };
    Ok(())
  }
}

/// Attempts to convert a specifier to a file path. By default, uses the Url
/// crate's `to_file_path()` method, but falls back to try and resolve unix-style
/// paths on Windows.
///
// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.
// Lifted from code I wrote here:
// https://github.com/denoland/deno/blob/8702894feb480181040152a06e7c3eaf38619629/cli/util/path.rs#L85
pub fn url_to_file_path(specifier: &Url) -> Option<PathBuf> {
  if specifier.scheme() != "file" {
    return None;
  }

  match specifier.to_file_path() {
    Ok(path) => Some(path),
    Err(()) => {
      if cfg!(windows) {
        // This might be a unix-style path which is used in the tests even on Windows.
        // Attempt to see if we can convert it to a `PathBuf`. This code should be removed
        // once/if https://github.com/servo/rust-url/issues/730 is implemented.
        if specifier.scheme() == "file" && specifier.host().is_none() && specifier.port().is_none() && specifier.path_segments().is_some() {
          let path_str = specifier.path();
          String::from_utf8(percent_encoding::percent_decode(path_str.as_bytes()).collect())
            .ok()
            .map(PathBuf::from)
        } else {
          None
        }
      } else {
        None
      }
    }
  }
}

fn uri_to_file_path(uri: &Uri) -> Option<PathBuf> {
  url_to_file_path(&Url::parse(uri.as_str()).ok()?)
}

#[cfg(test)]
mod test {
  use kprint_platform::environment::*;
  use std::sync::atomic::AtomicBool;
  use std::sync::atomic::Ordering;
  use std::time::Duration;

  use kprint_async_runtime::future;
  use tower_lsp::lsp_types::MessageType;
  use tower_lsp::lsp_types::Position;
  use tower_lsp::lsp_types::Range;
  use tower_lsp::lsp_types::TextDocumentContentChangeEvent;
  use tower_lsp::lsp_types::TextDocumentIdentifier;
  use tower_lsp::lsp_types::TextDocumentItem;
  use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;

  use crate::environment::TestConfigFileBuilder;
  use crate::environment::TestEnvironment;
  use crate::environment::TestEnvironmentBuilder;
  use crate::plugins::PluginCache;

  use super::client::ClientTrait;
  use super::*;

  macro_rules! did_open {
    ($backend:expr, $uri:expr, $text:expr) => {
      $backend
        .did_open(DidOpenTextDocumentParams {
          text_document: TextDocumentItem {
            uri: $uri.clone(),
            language_id: "txt".to_string(),
            version: 0,
            text: $text.to_string(),
          },
        })
        .await;
    };
  }

  macro_rules! did_close {
    ($backend:expr, $uri:expr) => {
      $backend
        .did_close(DidCloseTextDocumentParams {
          text_document: TextDocumentIdentifier { uri: $uri.clone() },
        })
        .await;
    };
  }

  macro_rules! assert_format {
    ($backend:expr, $uri:expr, $expected:expr) => {
      let result = $backend
        .formatting(
          DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: $uri.clone() },
            options: Default::default(),
            work_done_progress_params: Default::default(),
          },
          CancellationToken::new(),
        )
        .await;
      assert_eq!(result.unwrap(), $expected);
    };
  }

  #[test]
  fn should_reject_a_config_override_that_is_a_stream() {
    // a `<(...)` process substitution hands its text over once and has no file
    // behind it for the server to read again
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dev/fd/63", r#"{ "includes": ["**/*.txt"] }"#)
      .build();
    environment.add_fifo_path("/dev/fd/63");

    let args = SessionOptions {
      config: Some(kprint_host_api::options::ConfigArg::PathOrUrl("/dev/fd/63".to_string())),
      ..Default::default()
    };
    let err = resolve_config_override(&args, &environment).err().unwrap();

    assert_eq!(
      err.to_string(),
      concat!(
        "Cannot use the configuration provided by --config (/dev/fd/63) with this sub command because it needs a ",
        "configuration file it can read again or write back to. Specify a file path instead (ex. --config dprint.json)."
      )
    );
  }

  #[test]
  fn should_resolve_a_config_override_that_is_a_file() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "includes": ["**/*.txt"] }"#)
      .build();

    let args = SessionOptions {
      config: Some(kprint_host_api::options::ConfigArg::PathOrUrl("dprint.json".to_string())),
      ..Default::default()
    };

    assert_eq!(resolve_config_override(&args, &environment).unwrap(), Some(PathBuf::from("/dprint.json")));
    assert_eq!(resolve_config_override(&SessionOptions::default(), &environment).unwrap(), None);
  }

  #[test]
  fn should_format_with_lsp() {
    let environment = TestEnvironmentBuilder::new()
      .add_remote_wasm_plugin()
      .add_remote_process_plugin()
      .with_default_config(|c| {
        c.add_remote_wasm_plugin()
          .add_remote_process_plugin()
          .add_includes("**/*.{txt,ts}")
          .add_excludes("ignored_file.txt")
          .add_excludes("ignored-dir");
      })
      .initialize()
      .build();
    environment.set_env_var("DPRINT_WASM_FORMAT_ENGINE", Some("interpreter"));
    environment.write_file(".gitignore", "gitignored_file.txt\ngitignored_dir").unwrap();

    environment.clone().run_in_runtime(async move {
      let (backend, recv_task, test_client) = setup_backend(environment.clone());
      let backend = Rc::new(backend);
      let run_test_task = kprint_async_runtime::spawn({
        let environment = environment.clone();
        async move {
          backend
            .initialize(InitializeParams {
              process_id: Some(std::process::id()),
              ..Default::default()
            })
            .await
            .unwrap();
          backend.initialized(InitializedParams {}).await;

          let file_uri: Uri = "file:///file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 7), Position::new(0, 7)),
              new_text: "_formatted".to_string()
            }])
          );

          // update the document
          backend
            .did_change(DidChangeTextDocumentParams {
              text_document: VersionedTextDocumentIdentifier {
                uri: file_uri.clone(),
                version: 1,
              },
              content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 7), Position::new(0, 7))),
                range_length: None,
                text: "_formatted".to_string(),
              }],
            })
            .await;

          // format again, but it should be formatted now
          assert_format!(backend, file_uri, None);

          // change the text to an error
          backend
            .did_change(DidChangeTextDocumentParams {
              text_document: VersionedTextDocumentIdentifier {
                uri: file_uri.clone(),
                version: 1,
              },
              content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 0), Position::new(0, 17))),
                range_length: None,
                text: "plugin: should_error".to_string(),
              }],
            })
            .await;
          assert_format!(backend, file_uri, None);
          assert_eq!(
            environment.take_stderr_messages(),
            vec!["Failed formatting 'file:///file.txt': Did error.".to_string()],
          );

          let mut handles = Vec::new();
          for i in 0..50 {
            let file_uri: Uri = format!("file:///file_{}.txt", i).parse().unwrap();
            let backend = backend.clone();
            handles.push(kprint_async_runtime::spawn(async move {
              let file_text = format!("testing_{}", i);
              backend
                .did_open(DidOpenTextDocumentParams {
                  text_document: TextDocumentItem {
                    uri: file_uri.clone(),
                    language_id: "txt".to_string(),
                    version: 0,
                    text: file_text.clone(),
                  },
                })
                .await;
              let result = backend
                .formatting(
                  DocumentFormattingParams {
                    text_document: TextDocumentIdentifier { uri: file_uri.clone() },
                    options: Default::default(),
                    work_done_progress_params: Default::default(),
                  },
                  CancellationToken::new(),
                )
                .await;
              assert_eq!(
                result.unwrap().unwrap(),
                vec![TextEdit {
                  range: Range::new(Position::new(0, file_text.len() as u32), Position::new(0, file_text.len() as u32),),
                  new_text: "_formatted".to_string()
                }]
              );

              // test closing too
              backend
                .did_close(DidCloseTextDocumentParams {
                  text_document: TextDocumentIdentifier { uri: file_uri },
                })
                .await;
            }))
          }

          // ensure nothing panicked
          let results = future::join_all(handles).await;
          for result in results {
            result.unwrap();
          }

          // ignores excluded files
          let file_uri: Uri = "file:///ignored_file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(backend, file_uri, None);

          // ignores gitignored files
          let file_uri: Uri = "file:///gitignored_file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(backend, file_uri, None);

          // ignores file in gitignored dir
          let file_uri: Uri = "file:///gitignored_dir/file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(backend, file_uri, None);

          // ignores excluded directory files
          let file_uri: Uri = "file:///ignored-dir/file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(backend, file_uri, None);

          // ignores non-included files
          let file_uri: Uri = "file:///file.txt_ps".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(backend, file_uri, None);

          // now update the config file to include it by removing the includes,
          // which it should in the range formatting
          {
            let mut config_file = TestConfigFileBuilder::new(environment.clone());
            config_file.add_remote_wasm_plugin().add_remote_process_plugin();
            environment.write_file("/dprint.json", &config_file.to_string()).unwrap();
          }

          // range formatting
          let result = backend
            .range_formatting(
              DocumentRangeFormattingParams {
                text_document: TextDocumentIdentifier { uri: file_uri.clone() },
                range: Range {
                  start: Position { line: 0, character: 1 },
                  end: Position { line: 0, character: 2 },
                },
                options: Default::default(),
                work_done_progress_params: Default::default(),
              },
              CancellationToken::new(),
            )
            .await;
          assert_eq!(
            result.unwrap().unwrap(),
            vec![TextEdit {
              range: Range::new(Position::new(0, 7), Position::new(0, 7)),
              new_text: "_formatted_process".to_string()
            }]
          );

          // cancellation via a drop
          let file_uri: Uri = "file:///file_cancellation.txt_ps".parse().unwrap();
          did_open!(backend, file_uri, "wait_cancellation");

          let token = Arc::new(CancellationToken::new());
          kprint_async_runtime::spawn({
            let backend = backend.clone();
            let token = token.clone();
            async move {
              let future = backend.formatting(
                DocumentFormattingParams {
                  text_document: TextDocumentIdentifier { uri: file_uri.clone() },
                  options: Default::default(),
                  work_done_progress_params: Default::default(),
                },
                CancellationToken::new(),
              );
              // this token's cancellation will drop the future which
              // will cancel the formatting request internally
              tokio::select! {
                _ = future => {},
                _ = token.cancelled() => {}
              }
            }
          });

          // give some time for the message to be sent
          tokio::time::sleep(Duration::from_millis(50)).await;
          assert!(backend.has_pending().await);
          token.cancel();
          // give some time for the message to be cancelled
          tokio::time::sleep(Duration::from_millis(50)).await;
          assert!(!backend.has_pending().await);

          // create a config file with associations
          {
            let mut config_file = TestConfigFileBuilder::new(environment.clone());
            config_file
              .add_remote_wasm_plugin()
              .add_remote_process_plugin()
              .add_config_section(
                "test-plugin",
                r#"{
                "associations": [
                  "**/*.{txt,txt_ps}",
                  "some_file_name",
                  "test-process-plugin-exact-file"
                ],
                "ending": "wasm"
              }"#,
              )
              .add_config_section(
                "testProcessPlugin",
                r#"{
                "associations": [
                  "**/*.{txt,txt_ps,other}",
                  "test-process-plugin-exact-file"
                ]
                "ending": "ps"
              }"#,
              );
            environment.write_file("/dprint.json", &config_file.to_string()).unwrap();
          }

          // format using it
          let file_uri: Uri = "file:///associations1.txt".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 4), Position::new(0, 4)),
              new_text: "_wasm_ps".to_string()
            }])
          );
          backend
            .did_change(DidChangeTextDocumentParams {
              text_document: VersionedTextDocumentIdentifier {
                uri: file_uri.clone(),
                version: 1,
              },
              content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 0), Position::new(0, 4))),
                range_length: None,
                text: "plugin: text6".to_string(),
              }],
            })
            .await;
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 13), Position::new(0, 13)),
              new_text: "_wasm_ps_wasm_ps_ps".to_string()
            }])
          );

          // try .txt_ps file, which should act the same as above because
          // of the associations
          let file_uri: Uri = "file:///associations1.txt_ps".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 4), Position::new(0, 4)),
              new_text: "_wasm_ps".to_string()
            }])
          );

          // try the .other extension
          let file_uri: Uri = "file:///dir/file.other".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 4), Position::new(0, 4)),
              new_text: "_ps".to_string()
            }])
          );

          // try the exact file with no extension
          let file_uri: Uri = "file:///dir/some_file_name".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 4), Position::new(0, 4)),
              new_text: "_wasm".to_string()
            }])
          );

          // now try this special file name
          let file_uri: Uri = "file:///dir/test-process-plugin-exact-file".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 4), Position::new(0, 4)),
              new_text: "_wasm_ps".to_string()
            }])
          );

          // rewrite config with an override for package.txt
          {
            let mut config_file = TestConfigFileBuilder::new(environment.clone());
            config_file.add_remote_wasm_plugin().add_config_section(
              "test-plugin",
              r#"{
                "ending": "base",
                "overrides": {
                  "files": "**/package.txt",
                  "ending": "package"
                }
              }"#,
            );
            environment.write_file("/dprint.json", &config_file.to_string()).unwrap();
          }

          let file_uri: Uri = "file:///package.txt".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 4), Position::new(0, 4)),
              new_text: "_package".to_string()
            }])
          );

          // now ensure formatting works with a sub folder config file that has different config
          {
            let mut config_file = TestConfigFileBuilder::new(environment.clone());
            config_file.add_remote_wasm_plugin().add_remote_process_plugin();
            environment.mk_dir_all("/other_config").unwrap();
            environment.write_file("/other_config/dprint.json", &config_file.to_string()).unwrap();
            let mut config_file = TestConfigFileBuilder::new(environment.clone());
            config_file
              .add_remote_wasm_plugin()
              .add_remote_process_plugin()
              .add_config_section("test-plugin", r#"{"ending": "asdf"}"#)
              .add_config_section("testProcessPlugin", r#"{"ending": "asdf_ps"}"#);
            environment.mk_dir_all("/other_config/sub").unwrap();
            environment.write_file("/other_config/sub/dprint.json", &config_file.to_string()).unwrap();
          }

          for _ in 0..2 {
            let file_uri: Uri = "file:///other_config/file.txt".parse().unwrap();
            did_open!(backend, file_uri, "text");
            assert_format!(
              backend,
              file_uri,
              Some(vec![TextEdit {
                range: Range::new(Position::new(0, 4), Position::new(0, 4)),
                new_text: "_formatted".to_string()
              }])
            );
            did_close!(backend, file_uri);
            // switching to a different config should still work fine
            let file_uri: Uri = "file:///other_config/sub/file.txt".parse().unwrap();
            did_open!(backend, file_uri, "text");
            assert_format!(
              backend,
              file_uri,
              Some(vec![TextEdit {
                range: Range::new(Position::new(0, 4), Position::new(0, 4)),
                new_text: "_asdf".to_string()
              }])
            );
            did_close!(backend, file_uri);

            // now try with a process plugin
            let file_uri: Uri = "file:///other_config/file.txt_ps".parse().unwrap();
            did_open!(backend, file_uri, "text");
            assert_format!(
              backend,
              file_uri,
              Some(vec![TextEdit {
                range: Range::new(Position::new(0, 4), Position::new(0, 4)),
                new_text: "_formatted_process".to_string()
              }])
            );
            did_close!(backend, file_uri);
            // switching to a different config should work fine as well
            let file_uri: Uri = "file:///other_config/sub/file.txt_ps".parse().unwrap();
            did_open!(backend, file_uri, "text");
            assert_format!(
              backend,
              file_uri,
              Some(vec![TextEdit {
                range: Range::new(Position::new(0, 4), Position::new(0, 4)),
                new_text: "_asdf_ps".to_string()
              }])
            );
            did_close!(backend, file_uri);
          }

          backend.shutdown().await.unwrap();
        }
      });

      try_join!(recv_task, run_test_task).unwrap();

      assert_eq!(
        test_client.take_messages(),
        vec![
          (
            MessageType::INFO,
            format!("kprint {} ({}-{})", environment.cli_display_version(), environment.os(), environment.cpu_arch())
          ),
          (MessageType::INFO, "Server ready.".to_string())
        ]
      );
    });
  }

  #[test]
  fn should_format_shebang_file_with_lsp() {
    let environment = TestEnvironmentBuilder::new()
      .add_remote_wasm_plugin()
      .with_default_config(|c| {
        c.add_remote_wasm_plugin().add_config_section(
          "shebangs",
          r##"{
            "#!/bin/sh": "txt"
          }"##,
        );
      })
      .initialize()
      .build();
    environment.set_env_var("DPRINT_WASM_FORMAT_ENGINE", Some("interpreter"));

    environment.clone().run_in_runtime(async move {
      let (backend, recv_task, test_client) = setup_backend(environment.clone());
      let backend = Rc::new(backend);
      let run_test_task = kprint_async_runtime::spawn({
        async move {
          backend
            .initialize(InitializeParams {
              process_id: Some(std::process::id()),
              ..Default::default()
            })
            .await
            .unwrap();
          backend.initialized(InitializedParams {}).await;

          // extensionless file with a matching shebang
          let file_uri: Uri = "file:///scripts/build".parse().unwrap();
          did_open!(backend, file_uri, "#!/bin/sh\ntext");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(1, 4), Position::new(1, 4)),
              new_text: "_formatted".to_string()
            }])
          );

          // extensionless file without a shebang
          let file_uri: Uri = "file:///scripts/notes".parse().unwrap();
          did_open!(backend, file_uri, "text");
          assert_format!(backend, file_uri, None);

          backend.shutdown().await.unwrap();
        }
      });

      try_join!(recv_task, run_test_task).unwrap();

      assert_eq!(
        test_client.take_messages(),
        vec![
          (
            MessageType::INFO,
            format!("kprint {} ({}-{})", environment.cli_display_version(), environment.os(), environment.cpu_arch())
          ),
          (MessageType::INFO, "Server ready.".to_string())
        ]
      );
    });
  }

  #[test]
  fn should_format_with_lsp_using_global_config() {
    let environment = TestEnvironmentBuilder::new()
      .add_remote_wasm_plugin()
      .with_global_config(|c| {
        c.add_remote_wasm_plugin().add_includes("**/*.txt").add_excludes("ignored_file.txt");
      })
      .initialize()
      .build();
    environment.set_env_var("DPRINT_WASM_FORMAT_ENGINE", Some("interpreter"));

    environment.clone().run_in_runtime(async move {
      let (backend, recv_task, test_client) = setup_backend(environment.clone());
      let backend = Rc::new(backend);
      let run_test_task = kprint_async_runtime::spawn({
        async move {
          backend
            .initialize(InitializeParams {
              process_id: Some(std::process::id()),
              ..Default::default()
            })
            .await
            .unwrap();
          backend.initialized(InitializedParams {}).await;

          // Test that global config is being used
          let file_uri: Uri = "file:///file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 7), Position::new(0, 7)),
              new_text: "_formatted".to_string()
            }])
          );

          // Test that excluded file is not formatted
          let ignored_uri: Uri = "file:///ignored_file.txt".parse().unwrap();
          did_open!(backend, ignored_uri, "testing");
          assert_format!(backend, ignored_uri, None);

          // Test that non-matching file is not formatted
          let other_uri: Uri = "file:///file.js".parse().unwrap();
          did_open!(backend, other_uri, "testing");
          assert_format!(backend, other_uri, None);

          backend.shutdown().await.unwrap();
        }
      });

      try_join!(recv_task, run_test_task).unwrap();

      assert_eq!(
        test_client.take_messages(),
        vec![
          (
            MessageType::INFO,
            format!("kprint {} ({}-{})", environment.cli_display_version(), environment.os(), environment.cpu_arch())
          ),
          (MessageType::INFO, "Server ready.".to_string())
        ]
      );
    });
  }

  #[test]
  fn should_format_with_lsp_using_config_override() {
    let environment = TestEnvironmentBuilder::new()
      .add_remote_wasm_plugin()
      // default config with a custom ending so we can tell which config is used
      .with_default_config(|c| {
        c.add_remote_wasm_plugin()
          .add_includes("**/*.txt")
          .add_config_section("test-plugin", r#"{"ending": "default"}"#);
      })
      // override config at a non-default path with a different ending
      .with_local_config("/custom/dprint.json", |c| {
        c.add_remote_wasm_plugin()
          .add_includes("**/*.txt")
          .add_config_section("test-plugin", r#"{"ending": "custom"}"#);
      })
      .initialize()
      .build();
    environment.set_env_var("DPRINT_WASM_FORMAT_ENGINE", Some("interpreter"));

    environment.clone().run_in_runtime(async move {
      // pass the override config path
      let (backend, recv_task, test_client) = setup_backend_with_config(environment.clone(), Some(PathBuf::from("/custom/dprint.json")));
      let backend = Rc::new(backend);
      let run_test_task = kprint_async_runtime::spawn({
        async move {
          backend
            .initialize(InitializeParams {
              process_id: Some(std::process::id()),
              ..Default::default()
            })
            .await
            .unwrap();
          backend.initialized(InitializedParams {}).await;

          // should format using the overridden config (ending "custom"), not the default (ending "default")
          let file_uri: Uri = "file:///custom/file.txt".parse().unwrap();
          did_open!(backend, file_uri, "testing");
          assert_format!(
            backend,
            file_uri,
            Some(vec![TextEdit {
              range: Range::new(Position::new(0, 7), Position::new(0, 7)),
              new_text: "_custom".to_string()
            }])
          );

          backend.shutdown().await.unwrap();
        }
      });

      try_join!(recv_task, run_test_task).unwrap();

      assert_eq!(
        test_client.take_messages(),
        vec![
          (
            MessageType::INFO,
            format!("kprint {} ({}-{})", environment.cli_display_version(), environment.os(), environment.cpu_arch())
          ),
          (MessageType::INFO, "Server ready.".to_string())
        ]
      );
    });
  }

  fn setup_backend(environment: TestEnvironment) -> (Backend<TestEnvironment>, JoinHandle<()>, Arc<TestClient>) {
    setup_backend_with_config(environment, None)
  }

  fn setup_backend_with_config(environment: TestEnvironment, config_override: Option<PathBuf>) -> (Backend<TestEnvironment>, JoinHandle<()>, Arc<TestClient>) {
    let plugin_cache = PluginCache::new(environment.clone());
    let plugin_resolver = Rc::new(PluginResolver::new(environment.clone(), plugin_cache));
    let (tx, rx) = mpsc::unbounded_channel();
    let test_client = Arc::new(TestClient::default());
    let client = ClientWrapper::new(test_client.clone());
    let policy = crate::editor_trust::lsp_editor_policy(&environment, client.clone());
    let recv_task = start_message_handler(&environment, &plugin_resolver, config_override, policy, rx);
    (Backend::new(client, environment, tx), recv_task, test_client)
  }

  #[test]
  fn editor_checks_untrusted_redirects_before_contacting_the_target() {
    let source = "https://plugins.dprint.dev/workspace-config.json";
    let target = "https://downloads.example.com/workspace-config.json";
    let environment = TestEnvironmentBuilder::with_remote_wasm_plugin()
      .write_file("/workspace/dprint.json", format!(r#"{{"extends":"{source}"}}"#))
      .build();
    environment.add_remote_file_redirect(source, target);
    environment.add_remote_file(target, br#"{"plugins":["https://plugins.dprint.dev/test-plugin.wasm"]}"#);
    environment.run_in_runtime(async {
      let (backend, recv_task, client) = setup_backend(environment.clone());
      client.allow.store(false, Ordering::Relaxed);
      let uri: Uri = "file:///workspace/file.txt".parse().unwrap();
      did_open!(backend, uri, "input");
      assert_format!(backend, uri, None);
      assert_eq!(environment.remote_file_download_count(source), 1);
      assert_eq!(environment.remote_file_download_count(target), 0);
      assert_eq!(client.approvals.lock().len(), 1);
      assert!(client.approvals.lock()[0].contains(target));
      environment.take_stderr_messages();
      backend.shutdown().await.unwrap();
      recv_task.await.unwrap();
    });
  }

  #[test]
  fn changing_process_plugin_configuration_requires_a_new_approval() {
    let environment = TestEnvironmentBuilder::with_initialized_remote_process_plugin().build();
    environment.run_in_runtime(async {
      let (backend, recv_task, client) = setup_backend(environment.clone());
      let uri: Uri = "file:///file.txt_ps".parse().unwrap();
      did_open!(backend, uri, "input");
      assert!(
        backend
          .formatting(
            DocumentFormattingParams {
              text_document: TextDocumentIdentifier { uri: uri.clone() },
              options: Default::default(),
              work_done_progress_params: Default::default(),
            },
            CancellationToken::new()
          )
          .await
          .unwrap()
          .is_some()
      );
      assert_eq!(client.approvals.lock().len(), 1);
      let config_path = environment.cwd().join("dprint.json");
      let config = environment.read_file(&config_path).unwrap();
      let mut value: serde_json::Value = serde_json::from_str(&config).unwrap();
      value["testProcessPlugin"] = serde_json::json!({"ending":"changed"});
      environment.write_file(&config_path, &value.to_string()).unwrap();
      client.allow.store(false, Ordering::Relaxed);
      assert_format!(backend, uri, None);
      assert_eq!(client.approvals.lock().len(), 2);
      environment.take_stderr_messages();
      backend.shutdown().await.unwrap();
      recv_task.await.unwrap();
    });
  }

  #[test]
  fn formats_only_cells_of_included_notebooks() {
    let environment = TestEnvironmentBuilder::with_remote_wasm_plugin()
      .with_local_config("/workspace/dprint.json", |config| {
        config
          .add_remote_wasm_plugin()
          .add_includes("**/*.ipynb")
          .add_excludes("ignored.ipynb")
          .add_config_section("test-plugin", r#"{"associations":["**/*.{ipynb,txt}"]}"#);
      })
      .initialize()
      .build();
    environment.run_in_runtime(async {
      let (backend, recv_task, _) = setup_backend(environment.clone());
      let initialized = backend
        .initialize(
          serde_json::from_value(serde_json::json!({
            "capabilities": {"notebookDocument": {"synchronization": {}}}
          }))
          .unwrap(),
        )
        .await
        .unwrap();
      assert!(initialized.capabilities.notebook_document_sync.is_some());

      let cell: Uri = "vscode-notebook-cell:/workspace/notebook.ipynb#1".parse().unwrap();
      let open_notebook = |file: &str| {
        serde_json::from_value(serde_json::json!({
          "notebookDocument": {
            "uri": file, "notebookType": "jupyter-notebook", "version": 1, "cells": [{"kind": 2, "document": cell}]
          },
          "cellTextDocuments": [{"uri": cell, "languageId": "txt", "version": 1, "text": "cell"}]
        }))
        .unwrap()
      };
      backend.notebook_did_open(open_notebook("file:///workspace/notebook.ipynb")).await;
      assert_format!(
        backend,
        cell,
        Some(vec![TextEdit::new(Range::new(Position::new(0, 4), Position::new(0, 4)), "_formatted".into())])
      );
      backend
        .notebook_did_close(
          serde_json::from_value(serde_json::json!({
            "notebookDocument": {"uri": "file:///workspace/notebook.ipynb"}, "cellTextDocuments": [{"uri": cell}]
          }))
          .unwrap(),
        )
        .await;
      assert_format!(backend, cell, None);

      backend.notebook_did_open(open_notebook("file:///workspace/ignored.ipynb")).await;
      assert_format!(backend, cell, None);
      environment.take_stderr_messages();
      backend.shutdown().await.unwrap();
      recv_task.await.unwrap();
    });
  }

  #[test]
  fn formats_untitled_documents_using_the_current_first_workspace_folder() {
    let environment = TestEnvironmentBuilder::with_remote_wasm_plugin()
      .with_local_config("/first/dprint.json", |config| {
        config.add_remote_wasm_plugin().add_config_section("test-plugin", r#"{"ending":"first"}"#);
      })
      .with_local_config("/second/dprint.json", |config| {
        config.add_remote_wasm_plugin().add_config_section("test-plugin", r#"{"ending":"second"}"#);
      })
      .initialize()
      .build();
    environment.run_in_runtime(async {
      let (backend, recv_task, _) = setup_backend(environment.clone());
      backend
        .initialize(
          serde_json::from_value(serde_json::json!({
            "capabilities": {},
            "workspaceFolders": [{"uri":"file:///first","name":"first"}, {"uri":"file:///second","name":"second"}]
          }))
          .unwrap(),
        )
        .await
        .unwrap();
      let uri: Uri = "untitled:Untitled-1".parse().unwrap();
      did_open!(backend, uri, "input");
      let edit = |ending: &str| Some(vec![TextEdit::new(Range::new(Position::new(0, 5), Position::new(0, 5)), ending.into())]);
      let first_file: Uri = "file:///first/file.txt".parse().unwrap();
      let second_file: Uri = "file:///second/file.txt".parse().unwrap();
      did_open!(backend, first_file, "input");
      did_open!(backend, second_file, "input");
      assert_format!(backend, first_file, edit("_first"));
      assert_format!(backend, second_file, edit("_second"));
      assert_format!(backend, uri, edit("_first"));
      backend
        .did_change_workspace_folders(
          serde_json::from_value(serde_json::json!({
            "event": {"removed":[{"uri":"file:///first","name":"first"}],"added":[]}
          }))
          .unwrap(),
        )
        .await;
      assert_format!(backend, uri, edit("_second"));
      environment.take_stderr_messages();
      backend.shutdown().await.unwrap();
      recv_task.await.unwrap();
    });
  }

  #[derive(Debug)]
  struct TestClient {
    logged_messages: Mutex<Vec<(MessageType, String)>>,
    approvals: Mutex<Vec<String>>,
    allow: AtomicBool,
  }

  impl Default for TestClient {
    fn default() -> Self {
      Self {
        logged_messages: Default::default(),
        approvals: Default::default(),
        allow: AtomicBool::new(true),
      }
    }
  }

  impl Drop for TestClient {
    fn drop(&mut self) {
      // If this panics that means the logged messages weren't inspected for a test.
      if !std::thread::panicking() {
        let logged_messages = self.logged_messages.lock().clone();
        assert_eq!(
          logged_messages,
          Vec::<(MessageType, String)>::new(),
          "should not have logged messages left on drop"
        );
      }
    }
  }

  impl TestClient {
    pub fn take_messages(&self) -> Vec<(MessageType, String)> {
      self.logged_messages.lock().drain(..).collect()
    }
  }

  #[kprint_async_runtime::async_trait(?Send)]
  impl ClientTrait for TestClient {
    fn log(&self, message_type: MessageType, message: String) {
      self.logged_messages.lock().push((message_type, message));
    }

    async fn approve(&self, message: String) -> bool {
      self.approvals.lock().push(message);
      self.allow.load(Ordering::Relaxed)
    }
  }
}
