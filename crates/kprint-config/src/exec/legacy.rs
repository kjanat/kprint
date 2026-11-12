//! Compatibility with references to the external exec plugin,
//! dprint-plugin-exec.
//!
//! Configuration files reference the exec plugin by its releases. dprint
//! serves every one of them with its built-in exec: whatever release or
//! checksum a reference names, nothing is downloaded, and a configuration's
//! `exec` table works without any reference at all. Nothing else about the
//! external plugin is the built-in's identity: the built-in has its own name,
//! is versioned with dprint, and has its own cache revision.

use crate::plugins::PluginSourceReference;
use crate::utils::PathSource;

/// The external exec plugin's name.
pub const PLUGIN_NAME: &str = "dprint-plugin-exec";

/// Whether the reference is an alias for the built-in exec formatter.
pub fn is_exec_plugin_reference(reference: &PluginSourceReference) -> bool {
  match &reference.path_source {
    PathSource::Npm(npm) => npm.specifier.name == "@dprint/exec",
    PathSource::Remote(remote) => {
      let path = remote.url.path();
      match remote.url.host_str() {
        Some("plugins.dprint.dev") => {
          let path = path.strip_prefix("/dprint").filter(|rest| rest.starts_with('/')).unwrap_or(path);
          (path.starts_with("/exec-") && path.ends_with(".json")) || path.starts_with("/dprint-plugin-exec/") || path.starts_with("/exec/")
        }
        Some("github.com") => path.starts_with("/dprint/dprint-plugin-exec/releases/"),
        Some("cdn.jsdelivr.net") => path.strip_prefix("/npm/").is_some_and(is_exec_package_path),
        Some("unpkg.com") => path.strip_prefix('/').is_some_and(is_exec_package_path),
        _ => false,
      }
    }
    // Local paths are checked by manifest identity before process setup.
    PathSource::Local(_) => false,
  }
}

fn is_exec_package_path(path: &str) -> bool {
  path
    .strip_prefix("@dprint/exec")
    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('@'))
}
