use std::collections::BTreeMap;
use std::sync::Arc;

use futures::StreamExt;
use kprint_configuration::ConfigKeyMap;
use kprint_plugin_types::CancellationToken;
use kprint_plugin_types::FormatError;
use kprint_plugin_types::HostFormatCallback;
use kprint_plugin_types::HostFormatRequest;
use kprint_plugin_types::HostFormatTarget;

const MAX_IN_FLIGHT: usize = 8;

pub(super) struct EmbeddedTextOutput<K> {
  pub text: Option<String>,
  pub errors: BTreeMap<K, String>,
}

pub(super) struct EmbeddedTextFormatter<'a, K> {
  resolved: &'a BTreeMap<K, Result<Option<String>, FormatError>>,
  pending: BTreeMap<K, HostFormatRequest>,
  errors: BTreeMap<K, String>,
  token: &'a Arc<dyn CancellationToken>,
}

impl<K: Ord> EmbeddedTextFormatter<'_, K> {
  /// The key identifies an occurrence and its formatting options across render passes.
  pub fn format(&mut self, key: K, text: &str, settings: impl FnOnce() -> (HostFormatTarget, ConfigKeyMap)) -> Option<String> {
    match self.resolved.get(&key) {
      Some(Ok(text)) => text.clone(),
      Some(Err(error)) => {
        self.errors.entry(key).or_insert_with(|| error.to_string());
        None
      }
      None => {
        self.pending.entry(key).or_insert_with(|| {
          let (target, override_config) = settings();
          HostFormatRequest {
            target,
            override_config,
            file_bytes: text.as_bytes().to_vec(),
            range: None,
            token: self.token.clone(),
          }
        });
        None
      }
    }
  }
}

/// Let a synchronous text renderer discover nested format requests, resolve them
/// concurrently, and replay the renderer with the resulting text.
pub(super) async fn format_embedded_text<K: Ord>(
  host: HostFormatCallback,
  token: &Arc<dyn CancellationToken>,
  mut render: impl FnMut(&mut EmbeddedTextFormatter<'_, K>) -> Result<Option<String>, FormatError>,
) -> Result<Option<EmbeddedTextOutput<K>>, FormatError> {
  let mut resolved = BTreeMap::new();
  loop {
    if token.is_cancelled() {
      return Ok(None);
    }
    let mut formatter = EmbeddedTextFormatter {
      resolved: &resolved,
      pending: BTreeMap::new(),
      errors: BTreeMap::new(),
      token,
    };
    let text = render(&mut formatter)?;
    if formatter.pending.is_empty() {
      return Ok(Some(EmbeddedTextOutput {
        text,
        errors: formatter.errors,
      }));
    }

    let results = futures::stream::iter(formatter.pending)
      .map(|(key, request)| {
        let format = host(request);
        async move {
          let result = format.await.and_then(|text| text.map(String::from_utf8).transpose().map_err(FormatError::from));
          (key, result)
        }
      })
      .buffer_unordered(MAX_IN_FLIGHT)
      .collect::<Vec<_>>();
    tokio::select! {
      _ = token.wait_cancellation() => return Ok(None),
      results = results => resolved.extend(results),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::cell::Cell;
  use std::rc::Rc;

  use kprint_plugin_types::NullCancellationToken;

  use super::*;

  #[tokio::test]
  async fn reuses_resolved_fragments_when_a_render_discovers_more() {
    let requests = Rc::new(Cell::new(0));
    let host: HostFormatCallback = Rc::new({
      let requests = requests.clone();
      move |request| {
        requests.set(requests.get() + 1);
        assert_eq!(request.target, HostFormatTarget::Language("javascript".into()));
        Box::pin(async move { Ok(Some(request.file_bytes.to_ascii_uppercase())) })
      }
    });
    let settings = || (HostFormatTarget::Language("javascript".into()), ConfigKeyMap::new());
    let token: Arc<dyn CancellationToken> = Arc::new(NullCancellationToken);
    let output = format_embedded_text(host, &token, |formatter| {
      let Some(first) = formatter.format(0, "first", settings) else {
        return Ok(None);
      };
      let Some(second) = formatter.format(1, "second", settings) else {
        return Ok(None);
      };
      Ok(Some(format!("{first} {second}")))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.text.as_deref(), Some("FIRST SECOND"));
    assert!(output.errors.is_empty());
    assert_eq!(requests.get(), 2);
  }
}
