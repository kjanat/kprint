use std::borrow::Cow;
use std::path::Display;
use std::path::Path;
use std::path::PathBuf;
use std::path::StripPrefixError;

/// A PathBuf that is guaranteed to be canonicalized.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CanonicalizedPathBuf {
  path: PathBuf,
}

impl CanonicalizedPathBuf {
  /// Constructs a path already canonicalized by the environment implementation.
  pub fn new(path: PathBuf) -> Self {
    CanonicalizedPathBuf { path }
  }

  #[doc(hidden)]
  pub fn new_for_testing(path: impl AsRef<Path>) -> CanonicalizedPathBuf {
    assert!(path.as_ref().starts_with("/") || path.as_ref().starts_with("C:\\") || path.as_ref().starts_with("V:\\") || path.as_ref().starts_with("\\?\\UNC"));
    CanonicalizedPathBuf::new(path.as_ref().to_path_buf())
  }

  pub fn into_path_buf(self) -> PathBuf {
    self.path
  }

  pub fn display(&self) -> Display<'_> {
    self.path.display()
  }

  pub fn starts_with(&self, other: &CanonicalizedPathBuf) -> bool {
    self.path.starts_with(&other.path)
  }

  pub fn to_string_lossy(&self) -> Cow<'_, str> {
    self.path.to_string_lossy()
  }

  pub fn strip_prefix(&self, base: &CanonicalizedPathBuf) -> Result<&Path, StripPrefixError> {
    self.path.strip_prefix(&base.path)
  }

  pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
    self.path.join(path)
  }

  pub fn parent(&self) -> Option<CanonicalizedPathBuf> {
    self.path.parent().map(|p| CanonicalizedPathBuf::new(p.to_path_buf()))
  }

  pub fn join_panic_relative(&self, path: impl AsRef<str>) -> CanonicalizedPathBuf {
    // this is to prevent an unresolved path from being created
    if path.as_ref().contains("../") {
      panic!("Cannot join to {} because the path had ../ ({}).", self.path.display(), path.as_ref());
    }

    CanonicalizedPathBuf::new(self.path.join(path.as_ref()))
  }
}

impl AsRef<Path> for CanonicalizedPathBuf {
  fn as_ref(&self) -> &Path {
    self.path.as_path()
  }
}

pub(crate) fn canonicalize_path_maybe_not_exists(
  mut path: &Path,
  canonicalize: impl Fn(&Path) -> std::io::Result<CanonicalizedPathBuf>,
) -> std::io::Result<CanonicalizedPathBuf> {
  let mut names = Vec::new();
  loop {
    match canonicalize(path) {
      Ok(canonicalized_path) => {
        let mut path = canonicalized_path.into_path_buf();
        for name in names.into_iter().rev() {
          path.push(name);
        }
        return Ok(CanonicalizedPathBuf::new(path));
      }
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
        let Some(name) = path.file_name() else {
          return Err(err);
        };
        names.push(name);
        path = match path.parent() {
          // a relative path's last parent is empty, which is the cwd
          Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
          Some(parent) => parent,
          None => return Err(err),
        };
      }
      Err(err) => return Err(err),
    }
  }
}

#[cfg(test)]
mod missing_path_tests {
  use super::*;

  #[test]
  fn preserves_missing_suffix_after_resolving_existing_ancestor() {
    let result = canonicalize_path_maybe_not_exists(Path::new("link/new/file.txt"), |path| {
      if path == Path::new("link") {
        Ok(CanonicalizedPathBuf::new(PathBuf::from("/real")))
      } else {
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
      }
    })
    .unwrap();
    assert_eq!(result.into_path_buf(), Path::new("/real").join("new/file.txt"));
  }

  #[test]
  fn preserves_permission_errors() {
    let err = canonicalize_path_maybe_not_exists(Path::new("a/file.txt"), |_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
  }
}
