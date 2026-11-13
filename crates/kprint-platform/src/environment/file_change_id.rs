use super::FileChangeId;
use std::path::Path;

#[allow(clippy::disallowed_methods)] // Implements the native environment filesystem boundary.
pub(super) fn file_change_id(path: &Path) -> Option<FileChangeId> {
  #[cfg(unix)]
  {
    from_metadata(&std::fs::metadata(path).ok()?, std::time::SystemTime::now())
  }
  #[cfg(not(unix))]
  {
    let _ = path;
    None
  }
}

#[cfg(unix)]
fn from_metadata(metadata: &std::fs::Metadata, now: std::time::SystemTime) -> Option<FileChangeId> {
  use std::os::unix::fs::MetadataExt;
  // Do not use sys_traits::changed(): its real implementation truncates ctime
  // to seconds. Also hash recent files to avoid trusting an unsettled timestamp.
  let seconds = u64::try_from(metadata.ctime()).ok()?;
  let nanos = metadata.ctime_nsec();
  let now = now.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
  if !metadata.is_file() || nanos <= 0 || nanos >= 1_000_000_000 || now.saturating_sub(seconds) < 2 {
    return None;
  }
  Some(FileChangeId {
    device: metadata.dev(),
    inode: metadata.ino(),
    changed_secs: metadata.ctime(),
    changed_nanos: nanos,
  })
}

#[cfg(all(test, unix))]
#[allow(clippy::disallowed_methods)] // Isolated native filesystem fixtures.
mod tests {
  use super::*;
  use std::fs;
  use std::os::unix::fs::MetadataExt;
  use std::time::Duration;
  use std::time::SystemTime;

  #[test]
  fn restored_mtime_and_atomic_replacement_change_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plugin");
    fs::write(&path, b"first").unwrap();
    let before = fs::metadata(&path).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3);
    let first = from_metadata(&before, later).unwrap();
    assert!(from_metadata(&before, SystemTime::now()).is_none());
    fs::write(&path, b"other").unwrap();
    fs::File::options()
      .write(true)
      .open(&path)
      .unwrap()
      .set_modified(before.modified().unwrap())
      .unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_ne!(first, from_metadata(&after, later).unwrap());
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, b"third").unwrap();
    fs::File::options()
      .write(true)
      .open(&replacement)
      .unwrap()
      .set_modified(before.modified().unwrap())
      .unwrap();
    fs::rename(replacement, &path).unwrap();
    let replaced = fs::metadata(&path).unwrap();
    assert_ne!(before.ino(), replaced.ino());
    assert_ne!(first, from_metadata(&replaced, later).unwrap());
    assert!(file_change_id(dir.path()).is_none());
  }
}
