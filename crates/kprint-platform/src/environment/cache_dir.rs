use std::io;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn resolve() -> io::Result<PathBuf> {
  #[allow(clippy::disallowed_methods)]
  resolve_with(|name| std::env::var(name).ok(), dirs::cache_dir, dirs::home_dir, std::env::current_dir)
}

fn resolve_with(
  get_env: impl Fn(&str) -> Option<String>,
  system_cache: impl FnOnce() -> Option<PathBuf>,
  home: impl FnOnce() -> Option<PathBuf>,
  cwd: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
  for name in ["KPRINT_CACHE_DIR", "DPRINT_CACHE_DIR"] {
    if let Some(value) = get_env(name).filter(|value| !value.trim().is_empty()) {
      let path = PathBuf::from(value);
      return if path.is_absolute() {
        Ok(path)
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
    return Ok(home.join(manager.cache_path()).join(".kprint"));
  }

  system_cache()
    .map(|path| path.join("kprint").join("cache"))
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

// Cache selection runs before NativeEnvironment can be constructed.
#[allow(clippy::disallowed_methods)]
fn project_package_manager(cwd: &Path) -> Option<PackageManager> {
  for directory in cwd.ancestors() {
    if let Some(manager) = std::fs::read(directory.join("package.json"))
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
      .unwrap(),
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
