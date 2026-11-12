use std::io;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use cap_fs_ext::DirExt;
use cap_fs_ext::OpenOptionsSyncExt;

use super::CanonicalizedPathBuf;

pub(super) struct CacheLocation {
  path: PathBuf,
  explicit: bool,
}

impl CacheLocation {
  #[allow(clippy::disallowed_methods)]
  pub(super) fn prepare(self) -> io::Result<CanonicalizedPathBuf> {
    if self.explicit {
      std::fs::create_dir_all(&self.path)?;
      return dunce::canonicalize(self.path).map(CanonicalizedPathBuf::new);
    }
    let parent = self.path.parent().ok_or_else(|| io::Error::other("Cache directory has no parent"))?;
    std::fs::create_dir_all(parent)?;
    // Resolve relocated package-manager directories, while retaining the leaf
    // name so initialization rejects a linked leaf. The returned path requires
    // stable ancestors for the environment's lifetime; no directory handle is retained.
    let parent_path = dunce::canonicalize(parent)?;
    let parent = cap_std::fs::Dir::open_ambient_dir(&parent_path, cap_std::ambient_authority())?;
    let name = self.path.file_name().ok_or_else(|| io::Error::other("Cache directory has no name"))?;
    match parent.create_dir(name) {
      Ok(()) => {}
      Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
      Err(err) => return Err(err),
    }
    parent.open_dir_nofollow(name).map_err(|err| {
      io::Error::new(
        err.kind(),
        format!(
          "Automatic cache directory '{}' must be a real directory, not a symlink: {err}",
          self.path.display()
        ),
      )
    })?;
    Ok(CanonicalizedPathBuf::new(parent_path.join(name)))
  }
}

pub(super) fn resolve() -> io::Result<CacheLocation> {
  #[allow(clippy::disallowed_methods)]
  resolve_with(|name| std::env::var(name).ok(), dirs::cache_dir, dirs::home_dir, std::env::current_dir)
}

fn resolve_with(
  get_env: impl Fn(&str) -> Option<String>,
  system_cache: impl FnOnce() -> Option<PathBuf>,
  home: impl FnOnce() -> Option<PathBuf>,
  cwd: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<CacheLocation> {
  for name in ["KPRINT_CACHE_DIR", "DPRINT_CACHE_DIR"] {
    if let Some(value) = get_env(name).filter(|value| !value.trim().is_empty()) {
      let path = PathBuf::from(value);
      return if path.is_absolute() {
        Ok(CacheLocation { path, explicit: true })
      } else {
        Err(io::Error::new(
          io::ErrorKind::InvalidInput,
          format!("The {name} environment variable must specify an absolute path."),
        ))
      };
    }
  }

  if ["WORKERS_CI", "CF_PAGES"].iter().any(|name| get_env(name).as_deref() == Some("1")) {
    let manager = project_package_manager(&cwd()?)
      .or_else(|| get_env("npm_config_user_agent").and_then(|value| PackageManager::parse(value.split('/').next()?)))
      .unwrap_or(PackageManager::Npm);
    let home = home().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Expected to find home directory for Cloudflare build cache."))?;
    return Ok(CacheLocation {
      path: home.join(manager.cache_path()).join(".kprint"),
      explicit: false,
    });
  }

  system_cache()
    .map(|path| CacheLocation {
      path: path.join("kprint").join("cache"),
      explicit: false,
    })
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Expected to find cache directory."))
}

#[derive(Clone, Copy)]
enum PackageManager {
  Npm,
  Yarn,
  Pnpm,
  Bun,
}

impl PackageManager {
  fn parse(name: &str) -> Option<Self> {
    match name {
      "npm" => Some(Self::Npm),
      "yarn" => Some(Self::Yarn),
      "pnpm" => Some(Self::Pnpm),
      "bun" => Some(Self::Bun),
      _ => None,
    }
  }

  fn cache_path(self) -> &'static str {
    match self {
      Self::Npm => ".npm",
      Self::Yarn => ".cache/yarn",
      Self::Pnpm => ".pnpm-store",
      Self::Bun => ".bun/install/cache",
    }
  }
}

const MAX_PACKAGE_MANIFEST_BYTES: u64 = 1024 * 1024;

#[allow(clippy::disallowed_methods)] // Cache selection precedes the environment.
fn read_package_manifest(directory: &Path) -> io::Result<Vec<u8>> {
  let path = directory.join("package.json");
  // Check before opening, then check the opened handle too. Nonblocking open
  // prevents a concurrent replacement with a FIFO from hanging startup.
  let metadata = std::fs::metadata(&path)?;
  if !metadata.is_file() || metadata.len() > MAX_PACKAGE_MANIFEST_BYTES {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      "Package manifest must be a regular file of at most 1 MiB",
    ));
  }
  let mut options = cap_std::fs::OpenOptions::new();
  options.read(true).nonblock(true);
  let file = cap_std::fs::File::open_ambient_with(path, &options, cap_std::ambient_authority())?;
  let metadata = file.metadata()?;
  if !metadata.is_file() || metadata.len() > MAX_PACKAGE_MANIFEST_BYTES {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      "Package manifest must be a regular file of at most 1 MiB",
    ));
  }
  let mut bytes = Vec::new();
  file.take(MAX_PACKAGE_MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
  if bytes.len() as u64 > MAX_PACKAGE_MANIFEST_BYTES {
    return Err(io::Error::new(io::ErrorKind::InvalidData, "Package manifest exceeds 1 MiB"));
  }
  Ok(bytes)
}

// Cache selection runs before NativeEnvironment can be constructed.
#[allow(clippy::disallowed_methods)]
fn project_package_manager(cwd: &Path) -> Option<PackageManager> {
  for directory in cwd.ancestors() {
    if let Some(manager) = read_package_manifest(directory)
      .ok()
      .and_then(|source| serde_json::from_slice::<serde_json::Value>(&source).ok())
      .and_then(|manifest| PackageManager::parse(manifest.get("packageManager")?.as_str()?.split('@').next()?))
    {
      return Some(manager);
    }
    for (file, manager) in [
      ("bun.lock", PackageManager::Bun),
      ("bun.lockb", PackageManager::Bun),
      ("pnpm-lock.yaml", PackageManager::Pnpm),
      ("yarn.lock", PackageManager::Yarn),
      ("package-lock.json", PackageManager::Npm),
      ("npm-shrinkwrap.json", PackageManager::Npm),
    ] {
      if directory.join(file).is_file() {
        return Some(manager);
      }
    }
  }
  None
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // Tests create isolated filesystem fixtures before cache resolution.
mod tests {
  use super::*;

  fn resolve_test(root: &Path, vars: &[(&str, &str)]) -> io::Result<PathBuf> {
    resolve_with(
      |name| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()),
      || Some(root.join("system-cache")),
      || Some(root.join("home")),
      || Ok(root.join("project")),
    )
    .map(|location| location.path)
  }

  #[test]
  fn skips_invalid_and_oversized_manifests_and_uses_lockfiles() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("package.json");
    std::fs::write(root.path().join("pnpm-lock.yaml"), "").unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(project_package_manager(root.path()).unwrap().cache_path(), ".pnpm-store");
    std::fs::remove_dir(&path).unwrap();
    for contents in [b"invalid json".as_slice(), br#"{"packageManager":"unsupported@1"}"#] {
      std::fs::write(&path, contents).unwrap();
      assert_eq!(project_package_manager(root.path()).unwrap().cache_path(), ".pnpm-store");
    }
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_PACKAGE_MANIFEST_BYTES + 1).unwrap();
    assert!(read_package_manifest(root.path()).is_err());
    assert_eq!(project_package_manager(root.path()).unwrap().cache_path(), ".pnpm-store");
    std::fs::write(&path, br#"{"packageManager":"yarn@4"}"#).unwrap();
    assert_eq!(project_package_manager(root.path()).unwrap().cache_path(), ".cache/yarn");
  }

  #[cfg(unix)]
  #[test]
  fn accepts_symlinks_to_regular_manifests() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(root.path().join("manifest.json"), br#"{"packageManager":"yarn@4"}"#).unwrap();
    std::os::unix::fs::symlink("../manifest.json", project.join("package.json")).unwrap();
    assert_eq!(project_package_manager(&project).unwrap().cache_path(), ".cache/yarn");
  }

  #[cfg(unix)]
  #[test]
  fn fifo_manifest_does_not_block_startup() {
    const CHILD_VAR: &str = "KPRINT_TEST_FIFO_MANIFEST";
    if let Some(path) = std::env::var_os(CHILD_VAR) {
      assert_eq!(project_package_manager(Path::new(&path)).unwrap().cache_path(), ".pnpm-store");
      return;
    }
    use std::os::unix::ffi::OsStrExt;
    let root = tempfile::tempdir().unwrap();
    let path = std::ffi::CString::new(root.path().join("package.json").as_os_str().as_bytes()).unwrap();
    // SAFETY: path is a live, NUL-terminated pathname.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    std::fs::write(root.path().join("pnpm-lock.yaml"), "").unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "environment::cache_dir::tests::fifo_manifest_does_not_block_startup"])
      .env(CHILD_VAR, root.path())
      .spawn()
      .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
      if let Some(status) = child.try_wait().unwrap() {
        assert!(status.success());
        break;
      }
      if std::time::Instant::now() >= deadline {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("Cache selection blocked opening a FIFO manifest");
      }
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
  }

  #[cfg(unix)]
  #[test]
  fn automatic_cache_rejects_linked_leaf_and_accepts_relocated_parent() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let manager = root.path().join("manager");
    let target = root.path().join("unrelated");
    std::fs::create_dir(&manager).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep"), "unrelated data").unwrap();
    symlink(&target, manager.join(".kprint")).unwrap();
    assert!(
      CacheLocation {
        path: manager.join(".kprint"),
        explicit: false
      }
      .prepare()
      .is_err()
    );
    assert_eq!(std::fs::read_to_string(target.join("keep")).unwrap(), "unrelated data");
    let explicit = CacheLocation {
      path: manager.join(".kprint"),
      explicit: true,
    }
    .prepare()
    .unwrap();
    assert_eq!(explicit.as_ref(), dunce::canonicalize(&target).unwrap());
    std::fs::remove_file(manager.join(".kprint")).unwrap();
    std::fs::write(manager.join("keep"), "package-manager data").unwrap();
    let relocated = root.path().join("relocated");
    symlink(&manager, &relocated).unwrap();
    let cache = CacheLocation {
      path: relocated.join(".kprint"),
      explicit: false,
    }
    .prepare()
    .unwrap();
    std::fs::write(cache.join("plugin"), "cache data").unwrap();
    std::fs::remove_dir_all(cache.as_ref()).unwrap();
    assert_eq!(std::fs::read_to_string(manager.join("keep")).unwrap(), "package-manager data");
    assert!(target.join("keep").is_file());
  }

  #[test]
  fn a_prepared_cache_path_requires_unchanged_ancestors() {
    let root = tempfile::tempdir().unwrap();
    let ancestor = root.path().join("manager");
    let moved = root.path().join("original-manager");
    let cache = CacheLocation {
      path: ancestor.join(".kprint"),
      explicit: false,
    }
    .prepare()
    .unwrap();
    std::fs::write(cache.join("original"), b"original cache").unwrap();
    std::fs::rename(&ancestor, &moved).unwrap();
    std::fs::create_dir_all(ancestor.join(".kprint")).unwrap();
    std::fs::write(ancestor.join(".kprint/sentinel"), b"unrelated replacement").unwrap();

    // Selection is stable, but a path follows the replacement hierarchy.
    // Demonstrate that boundary without deleting the unrelated fixture.
    assert_eq!(std::fs::read(cache.join("sentinel")).unwrap(), b"unrelated replacement");
    assert_eq!(std::fs::read(moved.join(".kprint/original")).unwrap(), b"original cache");
    assert!(!cache.join("original").exists());
  }

  #[test]
  fn selection_is_project_specific_and_recovers_after_errors() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join("pnpm-lock.yaml"), "").unwrap();
    std::fs::write(second.join("yarn.lock"), "").unwrap();
    let select = |cwd: io::Result<PathBuf>| {
      resolve_with(
        |name| (name == "CF_PAGES").then(|| "1".into()),
        || None,
        || Some(root.path().join("home")),
        || cwd,
      )
    };
    assert!(select(Err(io::Error::other("cwd unavailable"))).is_err());
    assert_eq!(select(Ok(first)).unwrap().path, root.path().join("home/.pnpm-store/.kprint"));
    assert_eq!(select(Ok(second)).unwrap().path, root.path().join("home/.cache/yarn/.kprint"));
  }

  #[test]
  fn lockfile_precedence_is_bun_pnpm_yarn_npm() {
    let root = tempfile::tempdir().unwrap();
    for lockfile in ["bun.lock", "pnpm-lock.yaml", "yarn.lock", "package-lock.json"] {
      std::fs::write(root.path().join(lockfile), "").unwrap();
    }
    for (lockfile, expected) in [
      ("bun.lock", ".bun/install/cache"),
      ("pnpm-lock.yaml", ".pnpm-store"),
      ("yarn.lock", ".cache/yarn"),
      ("package-lock.json", ".npm"),
    ] {
      assert_eq!(project_package_manager(root.path()).unwrap().cache_path(), expected);
      std::fs::remove_file(root.path().join(lockfile)).unwrap();
    }
  }

  #[test]
  fn separates_default_cache_and_only_enables_cloudflare_for_its_flags() {
    let root = tempfile::tempdir().unwrap();
    for vars in [
      vec![],
      vec![("CI", "true")],
      vec![("WORKERS_CI", "0"), ("CF_PAGES", "false")],
      vec![("KPRINT_CACHE_DIR", " "), ("DPRINT_CACHE_DIR", "")],
    ] {
      assert_eq!(resolve_test(root.path(), &vars).unwrap(), root.path().join("system-cache/kprint/cache"));
    }
  }

  #[test]
  fn explicit_overrides_win_and_invalid_primary_does_not_fall_through() {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().join("primary");
    let legacy = root.path().join("legacy");
    let primary = primary.to_str().unwrap();
    let legacy = legacy.to_str().unwrap();
    for flag in ["WORKERS_CI", "CF_PAGES"] {
      assert_eq!(
        resolve_test(root.path(), &[(flag, "1"), ("KPRINT_CACHE_DIR", primary), ("DPRINT_CACHE_DIR", legacy)]).unwrap(),
        PathBuf::from(primary)
      );
      assert_eq!(
        resolve_test(root.path(), &[(flag, "1"), ("KPRINT_CACHE_DIR", " "), ("DPRINT_CACHE_DIR", legacy)]).unwrap(),
        PathBuf::from(legacy)
      );
    }
    for name in ["KPRINT_CACHE_DIR", "DPRINT_CACHE_DIR"] {
      let error = resolve_test(root.path(), &[(name, "relative/path"), ("DPRINT_CACHE_DIR", legacy)]).unwrap_err();
      assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
      assert_eq!(error.to_string(), format!("The {name} environment variable must specify an absolute path."));
    }
    assert_eq!(
      resolve_with(
        |name| (name == "KPRINT_CACHE_DIR").then(|| primary.to_string()),
        || None,
        || None,
        || Err(io::Error::other("unavailable"))
      )
      .unwrap()
      .path,
      PathBuf::from(primary)
    );
  }

  #[test]
  fn cloudflare_uses_each_documented_package_manager_cache() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    for (manager, cache) in [("npm", ".npm"), ("yarn", ".cache/yarn"), ("pnpm", ".pnpm-store"), ("bun", ".bun/install/cache")] {
      std::fs::write(project.join("package.json"), format!(r#"{{"packageManager":"{manager}@1.0.0"}}"#)).unwrap();
      for flag in ["WORKERS_CI", "CF_PAGES"] {
        assert_eq!(
          resolve_test(root.path(), &[(flag, "1"), ("npm_config_user_agent", "other/1.0")]).unwrap(),
          root.path().join("home").join(cache).join(".kprint")
        );
      }
    }
  }

  #[test]
  fn discovers_workspace_lockfiles_from_nested_directories() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("packages/tool");
    std::fs::create_dir_all(&nested).unwrap();
    for (lockfile, cache) in [
      ("bun.lock", ".bun/install/cache"),
      ("bun.lockb", ".bun/install/cache"),
      ("pnpm-lock.yaml", ".pnpm-store"),
      ("yarn.lock", ".cache/yarn"),
      ("package-lock.json", ".npm"),
      ("npm-shrinkwrap.json", ".npm"),
    ] {
      std::fs::write(root.path().join(lockfile), "").unwrap();
      assert_eq!(project_package_manager(&nested).unwrap().cache_path(), cache);
      std::fs::remove_file(root.path().join(lockfile)).unwrap();
    }
    std::fs::write(root.path().join("bun.lock"), "").unwrap();
    std::fs::write(nested.join("package.json"), r#"{"packageManager":"pnpm@11.0.0"}"#).unwrap();
    assert_eq!(project_package_manager(&nested).unwrap().cache_path(), ".pnpm-store");
  }

  #[test]
  fn cloudflare_falls_back_to_invoking_package_manager_then_npm() {
    let root = tempfile::tempdir().unwrap();
    for (agent, cache) in [
      ("bun/1.3.0 npm/?", ".bun/install/cache"),
      ("pnpm/10.0.0 npm/?", ".pnpm-store"),
      ("unknown/1.0.0", ".npm"),
      ("", ".npm"),
    ] {
      assert_eq!(
        resolve_test(root.path(), &[("CF_PAGES", "1"), ("npm_config_user_agent", agent)]).unwrap(),
        root.path().join("home").join(cache).join(".kprint")
      );
    }
    assert!(resolve_with(|_| None, || None, || None, || Err(io::Error::other("unavailable"))).is_err());
    assert!(resolve_with(|name| (name == "WORKERS_CI").then(|| "1".into()), || None, || None, || Ok(root.path().into())).is_err());
  }
}
