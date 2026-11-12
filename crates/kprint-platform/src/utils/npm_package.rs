use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use flate2::read::GzDecoder;
use std::path::Path;
use std::path::PathBuf;
use tar::Archive;
pub trait NpmPackageEnvironment: crate::environment::PlatformEnvironment + crate::environment::FileSystemSys {}
impl<T: crate::environment::PlatformEnvironment + crate::environment::FileSystemSys> NpmPackageEnvironment for T {}

pub fn extract_tarball_to_dir(tarball_bytes: &[u8], dest_dir: &Path, environment: &impl NpmPackageEnvironment) -> Result<()> {
  use crate::utils::fs::get_atomic_path;

  if environment.path_exists(dest_dir) {
    return Ok(());
  }

  let temp_dir = get_atomic_path(environment, dest_dir);
  environment.mk_dir_all(&temp_dir)?;

  if let Err(err) = extract_tarball_to_dir_inner(tarball_bytes, &temp_dir, environment) {
    environment.try_remove_dir_all(&temp_dir);
    return Err(err);
  }

  match environment.rename(&temp_dir, dest_dir) {
    Ok(()) => Ok(()),
    Err(err) => {
      if environment.path_exists(dest_dir) {
        // another concurrent extract finished first — discard our copy and
        // use theirs (immutable name@version means contents are equivalent)
        environment.try_remove_dir_all(&temp_dir);
        Ok(())
      } else {
        environment.try_remove_dir_all(&temp_dir);
        Err(err.into())
      }
    }
  }
}

/// Extracts an npm tarball into `dest_dir`, replacing any existing contents.
/// Same wrapper-stripping / path-traversal / permission-preserving rules as
/// [`extract_tarball_to_dir`], but for caches whose contents are *not*
/// content-addressable (e.g. the per-plugin cache, which gets rewritten
/// whenever the source plugin.json changes). The extract goes through a
/// sibling temp dir so a crash mid-extract can't leave the destination
/// half-populated; the caller is responsible for serializing extracts
/// against the same `dest_dir` via fs locks.
pub fn extract_tarball_replacing(tarball_bytes: &[u8], dest_dir: &Path, environment: &impl NpmPackageEnvironment) -> Result<()> {
  use crate::utils::fs::get_atomic_path;

  let temp_dir = get_atomic_path(environment, dest_dir);
  environment.mk_dir_all(&temp_dir)?;

  if let Err(err) = extract_tarball_to_dir_inner(tarball_bytes, &temp_dir, environment) {
    environment.try_remove_dir_all(&temp_dir);
    return Err(err);
  }

  // remove any existing directory before moving the staged extract into place.
  // surface a removal failure directly — otherwise the rename below fails with
  // a confusing "directory not empty" error that hides the real cause.
  if let Err(err) = environment.remove_dir_all(dest_dir) {
    environment.try_remove_dir_all(&temp_dir);
    return Err(err.into());
  }
  if let Err(err) = environment.rename(&temp_dir, dest_dir) {
    environment.try_remove_dir_all(&temp_dir);
    return Err(err.into());
  }
  Ok(())
}

#[doc(hidden)]
pub fn extract_tarball_to_dir_inner(tarball_bytes: &[u8], output_dir: &Path, environment: &impl NpmPackageEnvironment) -> Result<()> {
  let decoder = GzDecoder::new(tarball_bytes);
  let mut archive = Archive::new(decoder);
  // npm tarballs wrap every entry under a single top-level directory (usually
  // "package/"). Lock in that wrapper from the first entry and reject any
  // entry that doesn't share it, otherwise files outside the wrapper would
  // be silently dropped.
  let mut wrapper: Option<std::ffi::OsString> = None;
  let mut files_written: usize = 0;
  let mut created_dirs = std::collections::HashSet::from([output_dir.to_path_buf()]);

  for entry in archive.entries().context("Failed to read npm tarball entries")? {
    let mut entry = entry.context("Failed to read npm tarball entry")?;
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
