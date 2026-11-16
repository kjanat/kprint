use super::FileChangeId;
use std::path::Path;

#[allow(clippy::disallowed_methods)] // Implements the native environment filesystem boundary.
pub(super) fn file_change_id(path: &Path) -> Option<FileChangeId> {
  #[cfg(unix)]
  {
    from_metadata(&std::fs::metadata(path).ok()?, std::time::SystemTime::now()).filter(|_| keeps_change_times(path))
  }
  #[cfg(not(unix))]
  {
    let _ = path;
    None
  }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn statfs(path: &Path) -> Option<libc::statfs> {
  use std::os::unix::ffi::OsStrExt;
  let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
  let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
  // SAFETY: the path is NUL terminated, and `stats` is a statfs for it to write to
  if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
    return None;
  }
  // SAFETY: statfs succeeded, so it wrote `stats`
  Some(unsafe { stats.assume_init() })
}

// Network and FUSE filesystems report the change times their server or daemon chooses.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn keeps_change_times(path: &Path) -> bool {
  const EXT4: u32 = 0xEF53;
  const XFS: u32 = 0x5846_5342;
  const BTRFS: u32 = 0x9123_683E;
  const TMPFS: u32 = 0x0102_1994;
  const F2FS: u32 = 0xF2F5_2010;
  const BCACHEFS: u32 = 0xCA45_1A4E;
  const ZFS: u32 = 0x2FC1_2FC1;
  // f_type is a word wide and signed on some targets, and the magic numbers are 32 bits
  statfs(path).is_some_and(|stats| matches!(stats.f_type as u32, EXT4 | XFS | BTRFS | TMPFS | F2FS | BCACHEFS | ZFS))
}

#[cfg(target_os = "macos")]
fn keeps_change_times(path: &Path) -> bool {
  statfs(path).is_some_and(|stats| {
    // SAFETY: statfs writes f_fstypename NUL terminated
    let name = unsafe { std::ffi::CStr::from_ptr(stats.f_fstypename.as_ptr()) };
    name == c"apfs"
  })
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "macos"))))]
fn keeps_change_times(_path: &Path) -> bool {
  false
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

  fn seconds_after_change(metadata: &fs::Metadata, seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(metadata.ctime()).unwrap() + seconds)
  }

  #[test]
  fn restored_mtime_and_atomic_replacement_change_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plugin");
    fs::write(&path, b"first").unwrap();
    let before = fs::metadata(&path).unwrap();
    if before.ctime_nsec() == 0 {
      // this filesystem stores whole seconds
      assert!(from_metadata(&before, seconds_after_change(&before, 2)).is_none());
      return;
    }
    let first = from_metadata(&before, seconds_after_change(&before, 2)).unwrap();
    assert!(from_metadata(&before, seconds_after_change(&before, 1)).is_none());
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
    assert_ne!(first, from_metadata(&after, seconds_after_change(&after, 2)).unwrap());
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
    assert_ne!(first, from_metadata(&replaced, seconds_after_change(&replaced, 2)).unwrap());
    assert!(file_change_id(dir.path()).is_none());
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn trusts_the_change_times_of_local_filesystems_only() {
    assert!(keeps_change_times(std::path::Path::new("/dev/shm")));
    assert!(!keeps_change_times(std::path::Path::new("/proc/self/status")));
    assert!(!keeps_change_times(std::path::Path::new("/sys/kernel")));
    assert!(!keeps_change_times(std::path::Path::new("/nonexistent")));
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn trusts_the_change_times_of_apfs_only() {
    assert!(keeps_change_times(&std::env::temp_dir()));
    assert!(!keeps_change_times(std::path::Path::new("/dev")));
  }

  #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
  #[test]
  fn hashes_plugins_on_other_systems() {
    assert!(!keeps_change_times(&std::env::temp_dir()));
  }
}
