use crate::utils::replace_control_chars;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use flate2::read::GzDecoder;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use tar::Archive;
pub trait NpmPackageEnvironment: crate::environment::PlatformEnvironment + crate::environment::FileSystemSys {}
impl<T: crate::environment::PlatformEnvironment + crate::environment::FileSystemSys> NpmPackageEnvironment for T {}

/// Limits include skipped entries, tar metadata and padding.
#[derive(Clone, Debug)]
struct NpmExtractionOptions {
  pub max_entry_bytes: u64,
  pub max_expanded_bytes: u64,
  pub max_entries: usize,
  pub deadline: std::time::Instant,
  pub cancellation: tokio_util::sync::CancellationToken,
}

impl Default for NpmExtractionOptions {
  fn default() -> Self {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    Self {
      max_entry_bytes: 128 * 1024 * 1024,
      max_expanded_bytes: 512 * 1024 * 1024,
      max_entries: 100_000,
      deadline: crate::utils::current_deadline().map_or(deadline, |current| current.min(deadline)),
      cancellation: Default::default(),
    }
  }
}

impl NpmExtractionOptions {
  fn check(&self) -> std::io::Result<()> {
    if self.cancellation.is_cancelled() {
      return Err(std::io::Error::other("npm extraction cancelled"));
    }
    if std::time::Instant::now() >= self.deadline {
      return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "npm extraction deadline exceeded"));
    }
    Ok(())
  }
}

#[derive(Clone, Copy)]
pub enum NpmExtractionMode {
  Reuse,
  Replace,
}

/// Extracts with limits of 128 MiB per entry, 512 MiB decompressed data and
/// 100,000 headers, including skipped entries and metadata. The worker uses the
/// earlier of the caller's deadline and 30 seconds from invocation.
///
/// Dropping the future cancels the worker. Cancellation and deadlines are
/// checked between bounded reads and filesystem operations; an in-progress
/// filesystem call must return before the worker can clean up its staging.
pub async fn extract_npm_tarball(bytes: Vec<u8>, dest: PathBuf, environment: impl NpmPackageEnvironment, mode: NpmExtractionMode) -> Result<()> {
  extract_npm_tarball_with_options(bytes, dest, environment, mode, NpmExtractionOptions::default()).await
}

async fn extract_npm_tarball_with_options(
  bytes: Vec<u8>,
  dest: PathBuf,
  environment: impl NpmPackageEnvironment,
  mode: NpmExtractionMode,
  options: NpmExtractionOptions,
) -> Result<()> {
  let _cancel_on_drop = options.cancellation.clone().drop_guard();
  kprint_async_runtime::spawn_blocking(move || extract_with_options(&bytes, &dest, &environment, mode, &options)).await?
}

pub fn extract_tarball_to_dir(bytes: &[u8], dest: &Path, environment: &impl NpmPackageEnvironment) -> Result<()> {
  extract_with_options(bytes, dest, environment, NpmExtractionMode::Reuse, &NpmExtractionOptions::default())
}

pub fn extract_tarball_replacing(bytes: &[u8], dest: &Path, environment: &impl NpmPackageEnvironment) -> Result<()> {
  extract_with_options(bytes, dest, environment, NpmExtractionMode::Replace, &NpmExtractionOptions::default())
}

fn extract_with_options(
  bytes: &[u8],
  dest: &Path,
  environment: &impl NpmPackageEnvironment,
  mode: NpmExtractionMode,
  options: &NpmExtractionOptions,
) -> Result<()> {
  options.check()?;
  if matches!(mode, NpmExtractionMode::Reuse) && environment.path_exists(dest) {
    return Ok(());
  }
  let staging = crate::utils::fs::get_atomic_path(environment, dest);
  environment.mk_dir_all(&staging)?;
  let result = (|| {
    extract_inner(bytes, &staging, environment, options)?;
    options.check()?;
    if matches!(mode, NpmExtractionMode::Replace) {
      environment.remove_dir_all(dest)?;
    }
    match environment.rename(&staging, dest) {
      Ok(()) => Ok(()),
      Err(_) if matches!(mode, NpmExtractionMode::Reuse) && environment.path_exists(dest) => Ok(()),
      Err(err) => Err(err.into()),
    }
  })();
  // The staging path is absent after a successful rename. It is removed after
  // every failure, including a limit, cancellation or a concurrent cache fill.
  if environment.path_exists(&staging) {
    environment.try_remove_dir_all(&staging);
  }
  result
}

struct CheckedReader<'a, R> {
  inner: R,
  remaining: u64,
  options: &'a NpmExtractionOptions,
}

impl<R: std::io::Read> std::io::Read for CheckedReader<'_, R> {
  fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
    self.options.check()?;
    let size = buffer
      .len()
      .min(64 * 1024)
      .min(self.remaining.saturating_add(1).min(usize::MAX as u64) as usize);
    let read = self.inner.read(&mut buffer[..size])?;
    self.remaining = self
      .remaining
      .checked_sub(read as u64)
      .ok_or_else(|| std::io::Error::other("npm tarball exceeds byte limit"))?;
    Ok(read)
  }
}

fn archive_reader<'a>(bytes: &'a [u8], options: &'a NpmExtractionOptions) -> impl std::io::Read + 'a {
  let compressed = CheckedReader {
    inner: bytes,
    remaining: options.max_expanded_bytes,
    options,
  };
  CheckedReader {
    inner: GzDecoder::new(compressed),
    remaining: options.max_expanded_bytes,
    options,
  }
}

// A raw preflight counts extension headers and rejects oversized metadata before
// tar's normal iterator buffers GNU long names or PAX records. It also bounds
// skipped entries. Extraction then uses tar's normal path/extension handling.
fn preflight(bytes: &[u8], options: &NpmExtractionOptions) -> Result<()> {
  let mut archive = Archive::new(archive_reader(bytes, options));
  let mut pax_size = None;
  for (count, entry) in archive.entries()?.raw(true).enumerate() {
    options.check()?;
    if count >= options.max_entries {
      bail!("npm tarball exceeds entry count limit");
    }
    let mut entry = entry?;
    if entry.size() > options.max_entry_bytes {
      bail!("npm tarball entry exceeds per-entry byte limit");
    }
    let kind = entry.header().entry_type();
    if kind.is_gnu_sparse() {
      bail!("npm tarball contains an unsupported sparse entry");
    }
    if kind.is_pax_local_extensions() {
      if let Some(extensions) = entry.pax_extensions()? {
        for extension in extensions {
          options.check()?;
          let extension = extension?;
          if extension.key_bytes() == b"size" {
            if pax_size.is_some() {
              bail!("npm tarball has duplicate PAX size fields");
            }
            pax_size = Some(extension.value()?.parse::<u64>().context("Invalid npm tarball PAX size")?);
          }
        }
      }
    } else if !kind.is_gnu_longname() && !kind.is_gnu_longlink() && !kind.is_pax_global_extensions() {
      // Both passes must agree on entry boundaries. Otherwise a PAX size can
      // hide unvalidated extension headers inside a raw entry's payload.
      if let Some(size) = pax_size.take()
        && size != entry.size()
      {
        bail!("npm tarball PAX size disagrees with its entry header");
      }
    }
  }
  Ok(())
}

#[doc(hidden)]
pub fn extract_tarball_to_dir_inner(tarball_bytes: &[u8], output_dir: &Path, environment: &impl NpmPackageEnvironment) -> Result<()> {
  extract_inner(tarball_bytes, output_dir, environment, &NpmExtractionOptions::default())
}

fn extract_inner(tarball_bytes: &[u8], output_dir: &Path, environment: &impl NpmPackageEnvironment, options: &NpmExtractionOptions) -> Result<()> {
  options.check()?;
  preflight(tarball_bytes, options)?;
  let mut archive = Archive::new(archive_reader(tarball_bytes, options));
  // npm tarballs wrap every entry under a single top-level directory (usually
  // "package/"). Lock in that wrapper from the first entry and reject any
  // entry that doesn't share it, otherwise files outside the wrapper would
  // be silently dropped.
  let mut wrapper: Option<std::ffi::OsString> = None;
  let mut files_written: usize = 0;
  let mut created_dirs = std::collections::HashSet::from([output_dir.to_path_buf()]);

  for entry in archive.entries().context("Failed to read npm tarball entries")? {
    let mut entry = entry.context("Failed to read npm tarball entry")?;
    options.check()?;
    if entry.size() > options.max_entry_bytes {
      bail!("npm tarball entry exceeds per-entry byte limit");
    }
    let entry_type = entry.header().entry_type();

    // skip symlinks, hardlinks, and other special entries
    match entry_type {
      tar::EntryType::Regular | tar::EntryType::Directory => {}
      tar::EntryType::Symlink | tar::EntryType::Link => {
        // npm doesn't support symlinks/hardlinks in packages
        continue;
      }
      tar::EntryType::XGlobalHeader => continue,
      _ => continue,
    }

    let path = entry.path().context("Failed to get entry path")?.to_path_buf();

    // GNU tar prefixes entries with `./` by default; skip those leading
    // `CurDir`s so the wrapper-detection sees the real first directory.
    let mut components = path.components().peekable();
    while let Some(std::path::Component::CurDir) = components.peek() {
      components.next();
    }
    let Some(first) = components.next() else {
      continue;
    };
    // require a normal directory component as the wrapper. Absolute paths
    // (RootDir / Prefix) and `..` (ParentDir) at the top of an entry would
    // otherwise pin `wrapper` to a value we'd silently apply to subsequent
    // entries, mis-shaping the extract.
    let std::path::Component::Normal(first_name) = first else {
      bail!(
        "Refusing to extract npm tarball entry with non-relative top-level component: {}",
        path.display(),
      );
    };
    let first_os = first_name.to_os_string();
    match &wrapper {
      None => wrapper = Some(first_os),
      Some(existing) if existing == &first_os => {}
      Some(existing) => {
        bail!(
          "Inconsistent npm tarball: expected all entries under '{}/' but found '{}'",
          existing.to_string_lossy(),
          path.display(),
        );
      }
    }

    let relative: PathBuf = components.collect();
    if relative.as_os_str().is_empty() {
      continue;
    }

    let dest_path = output_dir.join(&relative);

    // path traversal check: ensure the resolved path stays within the output directory
    let normalized = normalize_path(&dest_path);
    if !normalized.starts_with(output_dir) {
      bail!("Refusing to extract tarball entry outside output directory: {}", path.display());
    }

    if entry_type == tar::EntryType::Directory {
      if created_dirs.insert(dest_path.clone()) {
        environment.mk_dir_all(&dest_path)?;
      }
      continue;
    }

    // regular file
    if let Some(parent) = dest_path.parent()
      && created_dirs.insert(parent.to_path_buf())
    {
      environment.mk_dir_all(parent)?;
    }

    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes)?;
    options.check()?;
    environment.write_file_bytes(&dest_path, &bytes)?;
    files_written += 1;

    // preserve executable permissions on unix
    #[cfg(unix)]
    if let Ok(mode) = entry.header().mode()
      && mode != 0o644
    {
      use sys_traits::FsSetPermissions;
      environment
        .fs_set_permissions(&dest_path, mode)
        .with_context(|| format!("Failed to set permissions on {}", dest_path.display()))?;
    }
  }

  if files_written == 0 {
    bail!("npm tarball contained no extractable files (expected at least one file under a wrapper directory)");
  }

  Ok(())
}

/// Normalizes a path by resolving `.` and `..` components without accessing the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
  let mut result = PathBuf::new();
  for component in path.components() {
    match component {
      std::path::Component::ParentDir => {
        result.pop();
      }
      std::path::Component::CurDir => {}
      other => result.push(other),
    }
  }
  result
}

thread_local! {
  static WARNED_DEPRECATED_NPM_PACKAGES: RefCell<HashSet<String>> = Default::default();
}

/// Warns when a packument says a version was deprecated, using the message its
/// publisher left (ex. that the package was renamed). Warns once per version
/// since resolving a version and then downloading it both read the packument.
pub fn warn_if_npm_deprecated(packument: &serde_json::Value, name: &str, version: &str, environment: &impl crate::environment::OutputEnvironment) {
  let Some(message) = deprecation_message(packument, version) else {
    return;
  };
  let package = format!("{}@{}", name, version);
  if WARNED_DEPRECATED_NPM_PACKAGES.with_borrow_mut(|warned| warned.insert(package.clone())) {
    // the message is whatever the publisher wrote, so don't let it send escape
    // sequences to the terminal
    log_warn!(environment, "npm package {} is deprecated: {}", package, replace_control_chars(message));
  }
}

/// The message a version was deprecated with, or `None` if it isn't deprecated.
/// An empty message is how npm un-deprecates a version.
fn deprecation_message<'a>(packument: &'a serde_json::Value, version: &str) -> Option<&'a str> {
  packument
    .get("versions")?
    .get(version)?
    .get("deprecated")?
    .as_str()
    .filter(|message| !message.is_empty())
}

#[cfg(test)]
mod extraction_tests {
  use super::*;
  use crate::environment::FileSystemEnvironment;
  use crate::environment::RealEnvironment;
  use kprint_test_support::test_helpers::create_test_npm_tarball;
  use std::io::Read;
  use std::io::Write;
  use std::time::Duration;
  use std::time::Instant;

  fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
  }

  fn header(kind: tar::EntryType, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_path("package/file").unwrap();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(0o644);
    header.set_cksum();
    header
  }

  fn fails_without_replacing(bytes: Vec<u8>, options: NpmExtractionOptions, expected: &'static str) {
    RealEnvironment::run_test_with_real_env(|environment| {
      Box::pin(async move {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("extracted");
        environment.mk_dir_all(&dest).unwrap();
        environment.write_file_bytes(dest.join("sentinel"), b"original").unwrap();
        let error = extract_with_options(&bytes, &dest, &environment, NpmExtractionMode::Replace, &options).unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
        assert_eq!(environment.read_file_bytes(dest.join("sentinel")).unwrap(), b"original");
        assert_eq!(environment.dir_info(&dest).unwrap().len(), 1);
        assert_eq!(environment.dir_info(root.path()).unwrap().len(), 1, "staging was not cleaned");
      })
    });
  }

  #[test]
  fn rejects_claimed_sizes_before_reading_payload_or_buffering_metadata() {
    for kind in [
      tar::EntryType::Regular,
      tar::EntryType::GNULongName,
      tar::EntryType::GNULongLink,
      tar::EntryType::XHeader,
    ] {
      // No payload exists. A size rejection must precede tar's metadata reads.
      let bytes = gzip(header(kind, 1 << 40).as_bytes());
      fails_without_replacing(
        bytes,
        NpmExtractionOptions {
          max_entry_bytes: 32,
          ..Default::default()
        },
        "per-entry byte limit",
      );
    }
  }

  #[test]
  fn bounds_decompressed_bytes_including_skipped_payloads_and_padding() {
    for kind in [tar::EntryType::Regular, tar::EntryType::Symlink] {
      let mut builder = tar::Builder::new(Vec::new());
      builder.append(&header(kind, 4096), &[0; 4096][..]).unwrap();
      let bytes = gzip(&builder.into_inner().unwrap());
      fails_without_replacing(
        bytes,
        NpmExtractionOptions {
          max_expanded_bytes: 1024,
          ..Default::default()
        },
        "byte limit",
      );
    }
    let bytes = create_test_npm_tarball(&[("package/file", b"x")]);
    fails_without_replacing(
      bytes,
      NpmExtractionOptions {
        max_expanded_bytes: 600,
        ..Default::default()
      },
      "byte limit",
    );
  }

  #[test]
  fn counts_skipped_and_extension_headers() {
    for kind in [tar::EntryType::Directory, tar::EntryType::Symlink, tar::EntryType::GNULongName] {
      let mut builder = tar::Builder::new(Vec::new());
      builder.append(&header(kind, 0), &[][..]).unwrap();
      builder.append(&header(tar::EntryType::Regular, 1), &b"x"[..]).unwrap();
      fails_without_replacing(
        gzip(&builder.into_inner().unwrap()),
        NpmExtractionOptions {
          max_entries: 1,
          ..Default::default()
        },
        "entry count limit",
      );
    }
  }

  #[test]
  fn rejects_pax_boundary_disagreement_and_duplicate_sizes() {
    for extension in ["9 size=0\n", "9 size=0\n9 size=1\n"] {
      let mut builder = tar::Builder::new(Vec::new());
      builder
        .append(&header(tar::EntryType::XHeader, extension.len() as u64), extension.as_bytes())
        .unwrap();
      builder.append(&header(tar::EntryType::Regular, 1), &b"x"[..]).unwrap();
      let expected = if extension.lines().count() == 1 {
        "PAX size disagrees"
      } else {
        "duplicate PAX size"
      };
      fails_without_replacing(gzip(&builder.into_inner().unwrap()), NpmExtractionOptions::default(), expected);
    }
  }

  #[test]
  fn rejects_sparse_entries_before_their_extension_blocks_are_parsed() {
    fails_without_replacing(
      gzip(header(tar::EntryType::GNUSparse, 0).as_bytes()),
      NpmExtractionOptions::default(),
      "unsupported sparse entry",
    );
  }

  #[test]
  fn cleans_files_already_written_when_a_later_path_is_invalid() {
    let bytes = kprint_test_support::test_helpers::create_test_npm_tarball_raw_paths(&[
      ("package/first", b"written to staging"),
      ("package/../../outside", b"must not escape"),
    ]);
    fails_without_replacing(bytes, NpmExtractionOptions::default(), "outside output directory");
  }

  #[test]
  fn failed_first_install_removes_staging_and_destination() {
    RealEnvironment::run_test_with_real_env(|environment| {
      Box::pin(async move {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("extracted");
        let bytes = create_test_npm_tarball(&[("package/file", b"too large")]);
        let options = NpmExtractionOptions {
          max_entry_bytes: 1,
          ..Default::default()
        };
        assert!(extract_with_options(&bytes, &dest, &environment, NpmExtractionMode::Reuse, &options).is_err());
        assert_eq!(environment.dir_info(root.path()).unwrap().len(), 0);
      })
    });
  }

  #[test]
  fn extracts_long_names_and_matching_pax_sizes() {
    RealEnvironment::run_test_with_real_env(|environment| {
      Box::pin(async move {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("extracted");
        let name = "a".repeat(150);
        let mut builder = tar::Builder::new(Vec::new());
        builder
          .append_data(&mut header(tar::EntryType::Regular, 2), format!("package/{name}"), &b"ok"[..])
          .unwrap();
        builder.append(&header(tar::EntryType::XHeader, 9), &b"9 size=1\n"[..]).unwrap();
        builder.append(&header(tar::EntryType::Regular, 1), &b"x"[..]).unwrap();
        let bytes = gzip(&builder.into_inner().unwrap());
        extract_npm_tarball(bytes, dest.clone(), environment.clone(), NpmExtractionMode::Replace)
          .await
          .unwrap();
        assert_eq!(environment.read_file_bytes(dest.join(name)).unwrap(), b"ok");
        assert_eq!(environment.read_file_bytes(dest.join("file")).unwrap(), b"x");
        assert_eq!(environment.dir_info(root.path()).unwrap().len(), 1);
      })
    });
  }

  #[test]
  fn deadline_and_cancellation_preserve_existing_contents() {
    let bytes = create_test_npm_tarball(&[("package/file", b"x")]);
    let cancelled = NpmExtractionOptions::default();
    cancelled.cancellation.cancel();
    fails_without_replacing(bytes.clone(), cancelled, "cancelled");
    fails_without_replacing(
      bytes,
      NpmExtractionOptions {
        deadline: Instant::now(),
        ..Default::default()
      },
      "deadline exceeded",
    );
  }

  #[test]
  fn checks_cancellation_between_bounded_reads() {
    struct CancelAfterRead(tokio_util::sync::CancellationToken);
    impl Read for CancelAfterRead {
      fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        assert!(buffer.len() <= 64 * 1024);
        buffer.fill(0);
        self.0.cancel();
        Ok(buffer.len())
      }
    }
    let options = NpmExtractionOptions::default();
    let mut reader = CheckedReader {
      inner: CancelAfterRead(options.cancellation.clone()),
      remaining: u64::MAX,
      options: &options,
    };
    let mut bytes = Vec::new();
    assert!(reader.read_to_end(&mut bytes).unwrap_err().to_string().contains("cancelled"));
    assert!(bytes.len() <= 64 * 1024);
  }

  #[test]
  fn observes_deadline_after_a_read() {
    struct DelayedRead;
    impl Read for DelayedRead {
      fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(20));
        buffer.fill(0);
        Ok(buffer.len())
      }
    }
    let options = NpmExtractionOptions {
      deadline: Instant::now() + Duration::from_millis(10),
      ..Default::default()
    };
    let mut reader = CheckedReader {
      inner: DelayedRead,
      remaining: u64::MAX,
      options: &options,
    };
    assert_eq!(reader.read_to_end(&mut Vec::new()).unwrap_err().kind(), std::io::ErrorKind::TimedOut);
  }

  #[test]
  fn inherits_the_callers_earlier_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    runtime.block_on(async {
      let earlier = Instant::now() + Duration::from_secs(1);
      crate::utils::run_before_deadline(earlier, async {
        assert_eq!(NpmExtractionOptions::default().deadline, earlier);
      })
      .await
      .unwrap();
    });
  }

  #[test]
  fn dropping_the_future_cancels_a_queued_worker() {
    use std::future::Future;
    use std::task::Poll;
    let environment = crate::environment::TestEnvironment::new();
    let options = NpmExtractionOptions::default();
    let cancellation = options.cancellation.clone();
    let runtime = tokio::runtime::Builder::new_current_thread().max_blocking_threads(1).build().unwrap();
    let (release, blocked) = std::sync::mpsc::channel();
    let (started, ready) = std::sync::mpsc::channel();
    runtime.spawn_blocking(move || {
      started.send(()).unwrap();
      blocked.recv().unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.block_on(async {
      let mut extraction = Box::pin(extract_npm_tarball_with_options(
        create_test_npm_tarball(&[("package/file", b"x")]),
        PathBuf::from("/extracted"),
        environment.clone(),
        NpmExtractionMode::Reuse,
        options,
      ));
      std::future::poll_fn(|cx| {
        assert!(extraction.as_mut().poll(cx).is_pending());
        Poll::Ready(())
      })
      .await;
      assert!(!cancellation.is_cancelled());
      drop(extraction);
      assert!(cancellation.is_cancelled());
      release.send(()).unwrap();
    });
    drop(runtime); // waits for the queued blocking worker to exit
    assert!(!environment.path_exists(Path::new("/extracted")));
  }
}
