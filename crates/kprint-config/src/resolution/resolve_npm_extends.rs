use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::environment::CanonicalizedPathBuf;
use crate::environment::ConfigEnvironment as Environment;
use crate::utils::NpmPathSource;
use crate::utils::NpmSpecifier;
use crate::utils::PathSource;
use crate::utils::ResolvedFilePathWithBytes;
use crate::utils::parse_npm_specifier;

/// File names checked at the root of an npm package when it doesn't otherwise
/// say what its configuration file is.
const DEFAULT_CONFIG_FILE_NAMES: [&str; 3] = ["dprint.json", "dprint.jsonc", "dprint.toml"];

/// Resolves the npm specifiers in the `extends` of a configuration file and
/// of everything it extends.
///
/// Remembers what it reads so each package and package.json is only
/// looked at once for a configuration file no matter how many times it's used.
#[derive(Default)]
pub struct NpmExtendsResolver {
  pub reload: bool,
  /// Directories in the npm cache of the registry packages that were resolved.
  registry_package_dirs: RefCell<HashMap<RegistryPackageKey, RegistryPackageDir>>,
  package_jsons: RefCell<HashMap<PathBuf, Rc<Option<serde_json::Value>>>>,
}

#[derive(PartialEq, Eq, Hash)]
struct RegistryPackageKey {
  /// Directory the registry is resolved from.
  base_dir: Option<PathBuf>,
  name: String,
  version: String,
}

impl NpmExtendsResolver {
  /// Resolves an `npm:` specifier in a configuration file's `extends` property
  /// to the configuration file in the npm package. Supported formats:
  /// - `npm:@scope/name` (node_modules, the package's main configuration file)
  /// - `npm:@scope/name/sub/path` (node_modules, an export or file in the package)
  /// - `npm:@scope/name@version` (registry, the package's main configuration file)
  /// - `npm:@scope/name@version/sub/path` (registry, an export or file in the package)
  ///
  /// A configuration file found in node_modules is a local file the same as if
  /// it was referenced by its path, while one downloaded from the registry is
  /// considered remote configuration.
  ///
  /// A checksum (ex. `npm:@scope/name@version/sub/path@<checksum>`) is verified
  /// against the SHA-256 checksum of the package's tarball the same as for npm
  /// plugins, so it covers every file in the package.
  pub async fn resolve(&self, text: &str, base: &PathSource, environment: &impl Environment) -> Result<ResolvedFilePathWithBytes> {
    let parsed = parse_npm_specifier(text)?;
    let checksum = parsed.checksum;
    let name = parsed.specifier.name;
    let sub_path = parsed.path_was_explicit.then_some(parsed.specifier.path);

    match parsed.specifier.version {
      Some(version) => {
        let base_dir = match base {
          PathSource::Local(local) => Some(local.path.clone()),
          PathSource::Npm(npm) => npm.base_dir.clone(),
          PathSource::Remote(_) => None,
        };
        let package = self
          .ensure_registry_package(&name, &version, base_dir.as_ref(), checksum.as_deref(), environment)
          .await?;
        if let Some(checksum) = &checksum
          && package.tarball_checksum != checksum.to_ascii_lowercase()
        {
          bail!(
            "Invalid checksum for npm package {}@{}.\n\nActual: {}\nExpected: {}",
            name,
            version,
            package.tarball_checksum,
            checksum,
          );
        }
        let path = self.resolve_config_path_in_package(&package.dir, &name, sub_path.as_deref(), environment)?;
        let content = read_package_file(&package.dir, &name, &path, environment)?;
        Ok(ResolvedFilePathWithBytes {
          source: PathSource::new_npm(
            NpmSpecifier {
              name,
              version: Some(version),
              path,
            },
            base_dir,
          ),
          is_first_download: package.is_first_download,
          content,
        })
      }
      None => {
        if checksum.is_some() {
          // a package in node_modules is local configuration, which doesn't need one
          bail!(
            "A checksum can only be specified for an npm specifier with a version (ex. npm:{}@x.x.x) in \"extends\": {}",
            name,
            text,
          );
        }
        let PathSource::Local(local_base) = base else {
          bail!(
            concat!(
              "Cannot resolve {} from node_modules because the configuration file extending it is not a local file ({}). ",
              "Specify a version to resolve it from the npm registry (ex. npm:{}@x.x.x)."
            ),
            text,
            base.display(),
            name,
          );
        };
        let Some(package_dir) = find_package_in_node_modules(&name, local_base.path.as_ref(), environment) else {
          bail!(
            concat!(
              "Could not find {} in node_modules. Make sure the package is installed (ex. npm install {}) ",
              "or specify a version to resolve it from the npm registry (ex. npm:{}@x.x.x)."
            ),
            name,
            name,
            name,
          );
        };
        let path = self.resolve_config_path_in_package(&package_dir, &name, sub_path.as_deref(), environment)?;
        let file_path = get_existing_package_file_path(&package_dir, &name, &path, environment)?;
        let file_path = environment.canonicalize(file_path)?;
        let content = environment.read_file_bytes(&file_path)?;
        Ok(ResolvedFilePathWithBytes {
          source: PathSource::new_local(file_path),
          is_first_download: false,
          content,
        })
      }
    }
  }

  /// Resolves a relative path in the `extends` property of a configuration file
  /// that came from an npm registry package to another file in that package.
  pub async fn resolve_relative(&self, relative_path: &str, base: &NpmPathSource, environment: &impl Environment) -> Result<ResolvedFilePathWithBytes> {
    let specifier = &base.specifier;
    let Some(version) = &specifier.version else {
      // configuration files from node_modules are local files
      bail!(
        "Cannot resolve a relative path against an npm specifier without a version: {}",
        specifier.display()
      );
    };
    let path = join_package_path(&specifier.path, relative_path)
      .with_context(|| format!("Failed resolving '{}' in the \"extends\" of {}", relative_path, specifier.display()))?;
    let package = self
      .ensure_registry_package(&specifier.name, version, base.base_dir.as_ref(), None, environment)
      .await?;
    let content = read_package_file(&package.dir, &specifier.name, &path, environment)?;
    Ok(ResolvedFilePathWithBytes {
      source: PathSource::new_npm(
        NpmSpecifier {
          name: specifier.name.clone(),
          version: Some(version.clone()),
          path,
        },
        base.base_dir.clone(),
      ),
      is_first_download: package.is_first_download,
      content,
    })
  }

  /// Gets the SHA-256 checksum of the tarball of the registry package that a
  /// resolved configuration file is in.
  pub fn tarball_checksum(&self, source: &NpmPathSource) -> Option<String> {
    let key = RegistryPackageKey {
      base_dir: source.base_dir.as_ref().map(|d| -> PathBuf { d.as_ref().to_path_buf() }),
      name: source.specifier.name.clone(),
      version: source.specifier.version.clone()?,
    };
    self.registry_package_dirs.borrow().get(&key).map(|package| package.tarball_checksum.clone())
  }

  /// Gets the directory of a registry package in the npm cache, downloading
  /// the package when it's not there.
  async fn ensure_registry_package(
    &self,
    name: &str,
    version: &str,
    base_dir: Option<&CanonicalizedPathBuf>,
    checksum: Option<&str>,
    environment: &impl Environment,
  ) -> Result<RegistryPackageDir> {
    let base_dir = base_dir.map(|d| -> &Path { d.as_ref() });
    let key = RegistryPackageKey {
      base_dir: base_dir.map(|d| d.to_path_buf()),
      name: name.to_string(),
      version: version.to_string(),
    };
    if let Some(package) = self.registry_package_dirs.borrow().get(&key)
      && checksum.is_none_or(|expected| package.tarball_checksum.eq_ignore_ascii_case(expected))
    {
      return Ok(RegistryPackageDir {
        is_first_download: false,
        ..package.clone()
      });
    }
    let file_name = name.rsplit('/').next().unwrap();
    let url = url::Url::parse(&format!("https://registry.npmjs.org/{name}/-/{file_name}-{version}.tgz"))?;
    let mut file = crate::utils::resolve_path_source_to_file_with_cache_mode(
      PathSource::new_remote(url.clone()),
      environment,
      if self.reload {
        crate::utils::RemoteCacheMode::Reload
      } else {
        crate::utils::RemoteCacheMode::Use
      },
    )
    .await?;
    let mut tarball_checksum = crate::utils::get_sha256_checksum(&file.content);
    if !self.reload && !file.is_first_download && checksum.is_some_and(|expected| !tarball_checksum.eq_ignore_ascii_case(expected)) {
      file = crate::utils::resolve_path_source_to_file_with_cache_mode(PathSource::new_remote(url), environment, crate::utils::RemoteCacheMode::Reload).await?;
      tarball_checksum = crate::utils::get_sha256_checksum(&file.content);
    }
    if let Some(expected) = checksum
      && !tarball_checksum.eq_ignore_ascii_case(expected)
    {
      bail!(
        "Invalid checksum for npm package {}@{}.\n\nActual: {}\nExpected: {}",
        name,
        version,
        tarball_checksum,
        expected
      );
    }
    if file.is_first_download || self.reload {
      // Deprecation is advisory. Cached configurations remain usable offline,
      // and a registry outage must not delay a fresh configuration indefinitely.
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
      let _ = crate::utils::run_before_deadline(deadline, async {
        let metadata_url = url::Url::parse(&format!("https://registry.npmjs.org/{name}"))?;
        let (_, metadata) = environment.download_file_err_404(&metadata_url).await?;
        let packument = serde_json::from_slice::<serde_json::Value>(&metadata.content)?;
        crate::utils::warn_if_npm_deprecated(&packument, name, version, environment);
        Ok::<_, anyhow::Error>(())
      })
      .await;
    }
    // Content-address the extraction so a refreshed package cannot reuse
    // files extracted from different bytes or an unrecorded old directory.
    let dir = environment.get_cache_dir().join("npm-config").join(&tarball_checksum);
    crate::utils::extract_tarball_to_dir(&file.content, &dir, environment)?;
    let package = RegistryPackageDir {
      dir,
      tarball_checksum,
      is_first_download: file.is_first_download,
    };
    self.registry_package_dirs.borrow_mut().insert(key, package.clone());
    Ok(package)
  }

  fn read_package_json(&self, package_dir: &Path, environment: &impl Environment) -> Result<Rc<Option<serde_json::Value>>> {
    if let Some(package_json) = self.package_jsons.borrow().get(package_dir) {
      return Ok(package_json.clone());
    }
    let package_json = Rc::new(read_package_json(package_dir, environment)?);
    self.package_jsons.borrow_mut().insert(package_dir.to_path_buf(), package_json.clone());
    Ok(package_json)
  }

  /// Gets the path within the package of the configuration file being referenced.
  ///
  /// The `exports` of the package's package.json have the highest precedence. When
  /// they don't have a match, a sub path is the path of a file in the package and
  /// no sub path is the package.json's `main` if it's a JSON file or otherwise a
  /// dprint.json file at the root of the package.
  ///
  /// Falling back when there's no match is more lenient than Node.js, which only
  /// allows what's in the `exports`. It's done because a package's `exports` are
  /// often only written for its JS and don't have its configuration files.
  fn resolve_config_path_in_package(&self, package_dir: &Path, package_name: &str, sub_path: Option<&str>, environment: &impl Environment) -> Result<String> {
    let package_json = self.read_package_json(package_dir, environment)?;
    let export_key = match sub_path {
      Some(sub_path) => format!("./{}", sub_path),
      None => ".".to_string(),
    };
    let export = match package_json.as_ref().as_ref().and_then(|p| p.get("exports")) {
      Some(exports) => resolve_export(exports, &export_key),
      None => ExportResolution::NotFound,
    };
    match export {
      ExportResolution::Target(target) => {
        let Some(path) = normalize_package_json_path(&target) else {
          bail!(
            "The \"exports\" of npm package {} has an invalid target for \"{}\": {}",
            package_name,
            export_key,
            target
          );
        };
        return Ok(path);
      }
      ExportResolution::Excluded => {
        bail!("The \"exports\" of npm package {} excludes \"{}\".", package_name, export_key);
      }
      ExportResolution::NotFound => {}
    }

    if let Some(sub_path) = sub_path {
      return Ok(sub_path.to_string());
    }

    let main = package_json
      .as_ref()
      .as_ref()
      .and_then(|p| p.get("main"))
      .and_then(|m| m.as_str())
      .filter(|m| is_json_file_name(m))
      .and_then(normalize_package_json_path)
      .filter(|m| environment.path_exists(package_dir.join(m)));
    if let Some(main) = main {
      return Ok(main);
    }

    for file_name in DEFAULT_CONFIG_FILE_NAMES {
      if environment.path_exists(package_dir.join(file_name)) {
        return Ok(file_name.to_string());
      }
    }

    bail!(
      concat!(
        "Could not determine the configuration file of npm package {}. Specify a file in the package ",
        "(ex. npm:{}/config.json) or have the package provide a dprint.json file, a \".\" entry ",
        "in its package.json's \"exports\", or a JSON file as its package.json's \"main\"."
      ),
      package_name,
      package_name,
    );
  }
}

#[derive(Clone)]
struct RegistryPackageDir {
  dir: PathBuf,
  /// SHA-256 of the tarball the directory was extracted from.
  tarball_checksum: String,
  /// Whether the package was downloaded from the registry instead of being
  /// found in the cache.
  is_first_download: bool,
}

fn read_package_json(package_dir: &Path, environment: &impl Environment) -> Result<Option<serde_json::Value>> {
  let path = package_dir.join("package.json");
  if !environment.path_exists(&path) {
    return Ok(None);
  }
  let text = environment.read_file(&path)?;
  let value = serde_json::from_str(&text).with_context(|| format!("Failed to parse {}", path.display()))?;
  Ok(Some(value))
}

#[derive(Debug, PartialEq, Eq)]
enum ExportResolution {
  /// Path of the file being exported (ex. `./src/index.json`).
  Target(String),
  /// The package explicitly doesn't export this (a `null` target).
  Excluded,
  NotFound,
}

/// Resolves a key (ex. `.` or `./sub/path`) in a package.json's `exports`
/// to its target following Node.js' resolution: an exact key has the highest
/// precedence, then the most specific subpath pattern (ex. `./configs/*`).
fn resolve_export(exports: &serde_json::Value, key: &str) -> ExportResolution {
  let sub_paths = match exports {
    serde_json::Value::Object(obj) if obj.keys().any(|k| k.starts_with('.')) => obj,
    // anything else is shorthand for the "." export
    _ if key == "." => return resolve_export_target(exports, None, false),
    _ => return ExportResolution::NotFound,
  };
  if !key.contains('*')
    && let Some(target) = sub_paths.get(key)
  {
    return resolve_export_target(target, None, false);
  }

  // the pattern with the longest text before the `*` wins, then the longest pattern
  let best_match = sub_paths
    .iter()
    .filter_map(|(pattern, target)| {
      let (prefix, suffix) = pattern.split_once('*')?;
      if suffix.contains('*') {
        return None;
      }
      let pattern_match = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
      (!pattern_match.is_empty()).then_some((prefix.len(), pattern.len(), pattern_match, target))
    })
    .max_by_key(|(prefix_len, pattern_len, _, _)| (*prefix_len, *pattern_len));
  match best_match {
    Some((_, _, pattern_match, target)) => resolve_export_target(target, Some(pattern_match), false),
    None => ExportResolution::NotFound,
  }
}

/// Resolves the target of an export. A target that's not within a `dprint`
/// condition is only used when it's a JSON file since the export is otherwise
/// most likely for a JS runtime (ex. `"default": "./index.js"`).
fn resolve_export_target(target: &serde_json::Value, pattern_match: Option<&str>, is_dprint_condition: bool) -> ExportResolution {
  match target {
    serde_json::Value::String(text) => {
      if is_dprint_condition || is_json_file_name(text) {
        ExportResolution::Target(match pattern_match {
          Some(pattern_match) => text.replace('*', pattern_match),
          None => text.to_string(),
        })
      } else {
        ExportResolution::NotFound
      }
    }
    serde_json::Value::Null => ExportResolution::Excluded,
    // the first matching condition that resolves wins
    serde_json::Value::Object(conditions) => conditions
      .iter()
      .filter_map(|(condition, target)| match condition.as_str() {
        "dprint" | "kprint" => Some(resolve_export_target(target, pattern_match, true)),
        "default" => Some(resolve_export_target(target, pattern_match, is_dprint_condition)),
        _ => None,
      })
      .find(|resolution| *resolution != ExportResolution::NotFound)
      .unwrap_or(ExportResolution::NotFound),
    // the first target that can be used wins
    serde_json::Value::Array(targets) => {
      let mut result = ExportResolution::NotFound;
      for target in targets {
        match resolve_export_target(target, pattern_match, is_dprint_condition) {
          ExportResolution::Target(target) => return ExportResolution::Target(target),
          ExportResolution::Excluded => result = ExportResolution::Excluded,
          ExportResolution::NotFound => {}
        }
      }
      result
    }
    _ => ExportResolution::NotFound,
  }
}

/// Converts a relative path in a package.json (ex. `./src/index.json`) to a
/// path within the package (ex. `src/index.json`), returning `None` when it's
/// not a path to a file within the package.
fn normalize_package_json_path(path: &str) -> Option<String> {
  join_package_path("", path).ok().filter(|path| !path.is_empty())
}

/// Resolves a relative path against the directory of a file in a package,
/// erroring when the result is outside the package.
pub(crate) fn join_package_path(from_file_path: &str, relative_path: &str) -> Result<String> {
  if relative_path.starts_with('/') || relative_path.contains('\\') || relative_path.contains(':') {
    bail!("Expected a relative path with forward slashes.");
  }
  let mut segments = from_file_path.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>();
  segments.pop(); // file name
  for segment in relative_path.split('/') {
    match segment {
      "" | "." => {}
      ".." => {
        if segments.pop().is_none() {
          bail!("The path is outside the npm package.");
        }
      }
      segment => segments.push(segment),
    }
  }
  Ok(segments.join("/"))
}

fn read_package_file(package_dir: &Path, package_name: &str, path: &str, environment: &impl Environment) -> Result<Vec<u8>> {
  let file_path = get_existing_package_file_path(package_dir, package_name, path, environment)?;
  Ok(environment.read_file_bytes(&file_path)?)
}

fn get_existing_package_file_path(package_dir: &Path, package_name: &str, path: &str, environment: &impl Environment) -> Result<PathBuf> {
  let file_path = package_dir.join(path);
  if !environment.path_is_file(&file_path) {
    bail!("Could not find {} in npm package {}.", path, package_name);
  }
  Ok(file_path)
}

fn is_json_file_name(file_name: &str) -> bool {
  let file_name = file_name.to_ascii_lowercase();
  file_name.ends_with(".json") || file_name.ends_with(".jsonc") || file_name.ends_with(".toml")
}

fn find_package_in_node_modules(name: &str, start: &Path, environment: &impl Environment) -> Option<PathBuf> {
  start
    .ancestors()
    .map(|dir| dir.join("node_modules").join(name))
    .find(|path| environment.path_exists(path))
}

#[cfg(test)]
mod tests {
  use pretty_assertions::assert_eq;

  use crate::configuration::ConfigMapValue;
  use crate::configuration::ResolvedConfig;
  use crate::configuration::resolve_config_from_args;
  use crate::environment::CanonicalizedPathBuf;
  use crate::environment::TestEnvironment;
  use crate::environment::TestEnvironmentBuilder;
  use crate::plugins::PluginSourceReference;
  use crate::test_helpers::create_test_npm_tarball;
  use kprint_configuration::ConfigKeyValue;
  use kprint_platform::environment::*;

  use super::*;

  #[test]
  fn should_extend_main_config_file_in_node_modules() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/project/dprint.json", r#"{ "extends": "npm:@scope/config", "prop1": { "value": 1 } }"#)
      .write_file(
        "/project/node_modules/@scope/config/dprint.json",
        r#"{
          "plugins": ["./test-plugin.json@checksum"],
          "prop1": { "value": 2 },
          "prop2": { "value": 3 }
        }"#,
      )
      .build();

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/project/dprint.json", &environment).await.unwrap();
      // it's a local file, so process plugins are kept and resolved relative to it
      assert_eq!(
        result.plugins.sources,
        vec![PluginSourceReference {
          path_source: PathSource::new_local(CanonicalizedPathBuf::new_for_testing("/project/node_modules/@scope/config/test-plugin.json")),
          checksum: Some("checksum".to_string()),
        }]
      );
      assert_eq!(get_number(&result, "prop1"), 1);
      assert_eq!(get_number(&result, "prop2"), 3);
      assert!(environment.take_stderr_messages().is_empty());
    });
  }

  #[test]
  fn should_extend_export_and_file_in_ancestor_node_modules() {
    let environment = TestEnvironmentBuilder::new()
      .write_file(
        "/project/sub/dprint.json",
        r#"{ "extends": ["npm:config", "npm:config/malva", "npm:config/other/file.json"] }"#,
      )
      .write_file(
        "/project/node_modules/config/package.json",
        r#"{
          "exports": {
            ".": { "import": "./index.js", "dprint": "./src/index.json" },
            "./malva": "./src/malva.json"
          }
        }"#,
      )
      .write_file(
        "/project/node_modules/config/src/index.json",
        r#"{ "prop1": { "value": 1 }, "extends": "./base.json" }"#,
      )
      .write_file("/project/node_modules/config/src/base.json", r#"{ "prop2": { "value": 2 } }"#)
      .write_file("/project/node_modules/config/src/malva.json", r#"{ "prop3": { "value": 3 } }"#)
      .write_file("/project/node_modules/config/other/file.json", r#"{ "prop4": { "value": 4 } }"#)
      .build();

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/project/sub/dprint.json", &environment).await.unwrap();
      assert_eq!(get_number(&result, "prop1"), 1);
      assert_eq!(get_number(&result, "prop2"), 2);
      assert_eq!(get_number(&result, "prop3"), 3);
      assert_eq!(get_number(&result, "prop4"), 4);
    });
  }

  #[test]
  fn should_follow_export_patterns_and_exclusions() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "extends": "npm:config/markdown" }"#)
      .write_file("/excluded.json", r#"{ "extends": "npm:config/internal/base.json" }"#)
      .write_file(
        "/node_modules/config/package.json",
        r#"{ "exports": { "./*": "./configs/*.json", "./internal/*": null } }"#,
      )
      .write_file("/node_modules/config/configs/markdown.json", r#"{ "prop1": { "value": 1 } }"#)
      .write_file("/node_modules/config/internal/base.json", r#"{ "prop2": { "value": 2 } }"#)
      .build();

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/dprint.json", &environment).await.unwrap();
      assert_eq!(get_number(&result, "prop1"), 1);
      assert_eq!(
        resolve_config("/excluded.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        "The \"exports\" of npm package config excludes \"./internal/base.json\".",
      );
    });
  }

  #[test]
  fn should_extend_package_json_main_in_node_modules() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "extends": "npm:config" }"#)
      .write_file("/node_modules/config/package.json", r#"{ "main": "index.json" }"#)
      .write_file("/node_modules/config/index.json", r#"{ "prop1": { "value": 1 } }"#)
      .build();

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/dprint.json", &environment).await.unwrap();
      assert_eq!(get_number(&result, "prop1"), 1);
    });
  }

  #[test]
  fn should_error_for_node_modules_problems() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/missing.json", r#"{ "extends": "npm:config" }"#)
      .write_file("/no-entry.json", r#"{ "extends": "npm:other" }"#)
      .write_file("/no-file.json", r#"{ "extends": "npm:other/file.json" }"#)
      .write_file("/checksum.json", r#"{ "extends": "npm:other/config.json@checksum" }"#)
      .write_file("/node_modules/other/package.json", r#"{ "main": "index.js" }"#)
      .write_file("/node_modules/other/config.json", "{}")
      .build();

    environment.clone().run_in_runtime(async move {
      assert_eq!(
        resolve_config("/missing.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        concat!(
          "Could not find config in node_modules. Make sure the package is installed (ex. npm install config) ",
          "or specify a version to resolve it from the npm registry (ex. npm:config@x.x.x)."
        ),
      );
      assert_eq!(
        resolve_config("/no-entry.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        concat!(
          "Could not determine the configuration file of npm package other. Specify a file in the package ",
          "(ex. npm:other/config.json) or have the package provide a dprint.json file, a \".\" entry ",
          "in its package.json's \"exports\", or a JSON file as its package.json's \"main\"."
        ),
      );
      assert_eq!(
        resolve_config("/no-file.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        "Could not find file.json in npm package other.",
      );
      assert_eq!(
        resolve_config("/checksum.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        "A checksum can only be specified for an npm specifier with a version (ex. npm:other@x.x.x) in \"extends\": npm:other/config.json@checksum",
      );
    });
  }

  #[test]
  fn should_trust_registry_config_extended_with_tarball_checksum() {
    // the tarball checksum pins every file in the package, so the files in it
    // that the config extends don't need their own checksum
    let environment = TestEnvironmentBuilder::new().build();
    let checksum = add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[
        ("package/config.json", r#"{ "extends": "./nested/base.json", "prop1": { "value": 1 } }"#),
        ("package/nested/base.json", r#"{ "prop2": { "value": 2 } }"#),
      ],
    );
    environment
      .write_file("/dprint.json", &format!(r#"{{ "extends": "npm:config@1.0.0/config.json@{checksum}" }}"#))
      .unwrap();

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/dprint.json", &environment).await.unwrap();
      assert_eq!(get_number(&result, "prop1"), 1);
      assert_eq!(get_number(&result, "prop2"), 2);
      assert_eq!(*result.plugins.trusted_config(), result.plugins.config);
      environment.take_stderr_messages();
    });
  }

  #[test]
  fn should_suggest_tarball_checksum_for_ignored_registry_config() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "extends": "npm:config@1.0.0/config.json" }"#)
      .build();
    let checksum = add_registry_package(&environment, "config", "1.0.0", &[("package/config.json", r#"{ "excludes": ["!**/*.txt"] }"#)]);

    environment.clone().run_in_runtime(async move {
      resolve_config("/dprint.json", &environment).await.unwrap();
      let stderr_messages = environment.take_stderr_messages();
      assert!(
        stderr_messages
          .iter()
          .any(|m| m.contains(&format!("extended with a checksum (npm:config@1.0.0/config.json@{checksum})"))),
        "{stderr_messages:?}"
      );
    });
  }

  #[test]
  fn should_error_when_registry_config_tarball_checksum_does_not_match() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "extends": "npm:config@1.0.0/config.json@checksum" }"#)
      .build();
    let checksum = add_registry_package(&environment, "config", "1.0.0", &[("package/config.json", "{}")]);

    environment.clone().run_in_runtime(async move {
      assert_eq!(
        resolve_config("/dprint.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        format!("Invalid checksum for npm package config@1.0.0.\n\nActual: {checksum}\nExpected: checksum"),
      );
    });
  }

  #[test]
  fn should_require_checksum_for_url_extended_by_registry_config_extended_with_checksum() {
    // only the files in the package are pinned by its tarball checksum
    let environment = TestEnvironmentBuilder::new().build();
    environment.add_remote_file_bytes("https://dprint.dev/other.json", br#"{ "prop1": { "value": 1 } }"#.to_vec());
    let checksum = add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[("package/config.json", r#"{ "extends": "https://dprint.dev/other.json" }"#)],
    );
    environment
      .write_file("/dprint.json", &format!(r#"{{ "extends": "npm:config@1.0.0/config.json@{checksum}" }}"#))
      .unwrap();

    environment.clone().run_in_runtime(async move {
      let err = resolve_config("/dprint.json", &environment).await.unwrap_err().to_string();
      assert!(err.starts_with("Expected a checksum for 'https://dprint.dev/other.json'"), "{err}");
    });
  }

  #[test]
  fn should_extend_from_registry_as_remote_config() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "extends": "npm:config@1.0.0", "prop1": { "value": 1 } }"#)
      .build();
    add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[
        ("package/package.json", r#"{ "exports": "./configs/index.json" }"#),
        (
          "package/configs/index.json",
          r#"{
            "extends": ["./nested/other.json", "npm:second@2.0.0/file.json"],
            "includes": ["**/*.ts"],
            "plugins": ["https://plugins.dprint.dev/test-plugin.wasm", "https://plugins.dprint.dev/test-plugin.json@checksum"],
            "prop1": { "value": 2 },
            "prop2": { "value": 2 }
          }"#,
        ),
        (
          "package/configs/nested/other.json",
          r#"{ "extends": "../../root.json", "prop3": { "value": 3 } }"#,
        ),
        ("package/root.json", r#"{ "prop4": { "value": 4 } }"#),
      ],
    );
    add_registry_package(&environment, "second", "2.0.0", &[("package/file.json", r#"{ "prop5": { "value": 5 } }"#)]);

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/dprint.json", &environment).await.unwrap();
      // remote configuration, so the includes and non-wasm plugins are ignored
      assert_eq!(result.files.includes, None);
      assert!(!result.plugins.config.contains_key("includes"));
      assert_eq!(
        result.plugins.sources,
        vec![PluginSourceReference::new_remote_from_str("https://plugins.dprint.dev/test-plugin.wasm")]
      );
      assert_eq!(get_number(&result, "prop1"), 1);
      assert_eq!(get_number(&result, "prop2"), 2);
      assert_eq!(get_number(&result, "prop3"), 3);
      assert_eq!(get_number(&result, "prop4"), 4);
      assert_eq!(get_number(&result, "prop5"), 5);
      assert_eq!(environment.take_stderr_messages().len(), 3);

      // uses the cache the second time
      for url in [
        "https://registry.npmjs.org/config",
        "https://registry.npmjs.org/config/-/config-1.0.0.tgz",
        "https://registry.npmjs.org/second",
        "https://registry.npmjs.org/second/-/second-2.0.0.tgz",
      ] {
        environment.add_remote_file_error(url, "must not be fetched again");
      }
      let second_result = resolve_config("/dprint.json", &environment).await.unwrap();
      assert_eq!(second_result.plugins.config, result.plugins.config);
      // only warns about the includes on the first download
      assert_eq!(environment.take_stderr_messages().len(), 1);
    });
  }

  #[test]
  fn should_error_for_registry_problems() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/escape.json", r#"{ "extends": "npm:config@1.0.0/escape.json" }"#)
      .write_file("/node-modules.json", r#"{ "extends": "npm:config@1.0.0/node-modules.json" }"#)
      .write_file("/missing-version.json", r#"{ "extends": "npm:config@2.0.0" }"#)
      .write_file("/home-dir.json", r#"{ "extends": "npm:config@1.0.0/home-dir.json" }"#)
      .write_file("/file-url.json", r#"{ "extends": "npm:config@1.0.0/file-url.json" }"#)
      .write_file("/drive-letter.json", r#"{ "extends": "npm:config@1.0.0/C:/local.json" }"#)
      .write_file("/local.json", r#"{ "plugins": ["./test-plugin.json@checksum"] }"#)
      .build();
    add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[
        ("package/home-dir.json", r#"{ "extends": "~/local.json" }"#),
        ("package/file-url.json", r#"{ "extends": "file:///local.json" }"#),
        ("package/escape.json", r#"{ "extends": "../other.json" }"#),
        ("package/node-modules.json", r#"{ "extends": "npm:other" }"#),
      ],
    );

    environment.clone().run_in_runtime(async move {
      assert_eq!(
        resolve_config("/escape.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        "Failed resolving '../other.json' in the \"extends\" of npm:config@1.0.0/escape.json: The path is outside the npm package.",
      );
      assert_eq!(
        resolve_config("/node-modules.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        concat!(
          "Cannot resolve npm:other from node_modules because the configuration file extending it is not a local file ",
          "(npm:config@1.0.0/node-modules.json). Specify a version to resolve it from the npm registry (ex. npm:other@x.x.x)."
        ),
      );
      assert_eq!(
        resolve_config("/missing-version.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        "Error downloading https://registry.npmjs.org/config/-/config-2.0.0.tgz - 404 Not Found",
      );
      // must never be able to extend a local file, since it would be considered local configuration
      assert_eq!(
        resolve_config("/home-dir.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        concat!(
          "Cannot extend '~/local.json' in a configuration file from the npm registry. ",
          "Only relative paths to files in the package, npm specifiers, and http(s) urls are supported."
        ),
      );
      assert_eq!(
        resolve_config("/file-url.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        concat!(
          "Cannot extend 'file:///local.json' in a configuration file from the npm registry. ",
          "Only relative paths to files in the package, npm specifiers, and http(s) urls are supported."
        ),
      );
      assert_eq!(
        resolve_config("/drive-letter.json", &environment)
          .await
          .unwrap_err()
          .to_string()
          .split("\n    at ")
          .next()
          .unwrap(),
        "Plugin path in npm specifier must not contain colons (got 'C:/local.json'): npm:config@1.0.0/C:/local.json",
      );
    });
  }

  #[test]
  fn should_resolve_npm_plugins_in_registry_config_from_extending_config_dir() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/project/dprint.json", r#"{ "extends": "npm:config@1.0.0" }"#)
      .build();
    add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[("package/dprint.json", r#"{ "plugins": ["npm:@dprint/typescript"] }"#)],
    );

    environment.clone().run_in_runtime(async move {
      let result = resolve_config("/project/dprint.json", &environment).await.unwrap();
      assert_eq!(
        result.plugins.sources,
        vec![PluginSourceReference {
          path_source: PathSource::new_npm(
            NpmSpecifier {
              name: "@dprint/typescript".to_string(),
              version: None,
              path: "plugin.wasm".to_string(),
            },
            Some(CanonicalizedPathBuf::new_for_testing("/project")),
          ),
          checksum: None,
        }]
      );
    });
  }

  #[test]
  fn should_consider_local_file_extended_by_remote_config_as_remote() {
    let local_config = r#"{
      "includes": ["**/*.ts"],
      "plugins": ["./test-plugin.json@checksum", "./test-plugin.wasm"],
      "prop1": { "value": 1 }
    }"#;
    let environment = TestEnvironmentBuilder::new()
      .write_file("/url.json", r#"{ "extends": "https://dprint.dev/config.json" }"#)
      .write_file("/npm.json", r#"{ "extends": "npm:config@1.0.0" }"#)
      .write_file("/local.json", r#"{ "extends": "./dir/local.json" }"#)
      .write_file("/home/local.json", local_config)
      .write_file("/dir/local.json", local_config)
      .build();
    environment.add_remote_file_bytes("https://dprint.dev/config.json", br#"{ "extends": "~/local.json" }"#.to_vec());
    add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[("package/dprint.json", r#"{ "extends": "https://dprint.dev/config.json" }"#)],
    );

    environment.clone().run_in_runtime(async move {
      for config_path in ["/url.json", "/npm.json"] {
        let result = resolve_config(config_path, &environment).await.unwrap();
        assert_eq!(get_number(&result, "prop1"), 1);
        // the includes and non-wasm plugins are ignored
        assert_eq!(result.files.includes, None);
        assert!(!result.plugins.config.contains_key("includes"));
        assert_eq!(
          result.plugins.sources,
          vec![PluginSourceReference {
            path_source: PathSource::new_local(CanonicalizedPathBuf::new_for_testing("/home/test-plugin.wasm")),
            checksum: None,
          }]
        );
        assert!(!environment.take_stderr_messages().is_empty());
      }

      // not when only extended by local config
      environment
        .write_file("/dir/local.json", &local_config.replace("\"includes\": [\"**/*.ts\"],", ""))
        .unwrap();
      let result = resolve_config("/local.json", &environment).await.unwrap();
      assert_eq!(result.plugins.sources.len(), 2);
      assert!(environment.take_stderr_messages().is_empty());
    });
  }

  #[test]
  fn should_error_for_circular_extends() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/self.json", r#"{ "extends": "./self.json" }"#)
      .write_file("/a.json", r#"{ "extends": "./dir/b.json" }"#)
      .write_file("/dir/b.json", r#"{ "extends": "npm:config@1.0.0" }"#)
      .write_file("/diamond.json", r#"{ "extends": ["./left.json", "./right.json"] }"#)
      .write_file("/left.json", r#"{ "extends": "./shared.json", "prop1": { "value": 1 } }"#)
      .write_file("/right.json", r#"{ "extends": "./shared.json", "prop2": { "value": 2 } }"#)
      .write_file("/shared.json", r#"{ "prop3": { "value": 3 } }"#)
      .build();
    add_registry_package(
      &environment,
      "config",
      "1.0.0",
      &[
        ("package/dprint.json", r#"{ "extends": "./other.json" }"#),
        ("package/other.json", r#"{ "extends": "npm:config@1.0.0/dprint.json" }"#),
      ],
    );

    environment.clone().run_in_runtime(async move {
      assert_eq!(
        resolve_config("/self.json", &environment).await.unwrap_err().to_string().split("\n    at ").next().unwrap(),
        "The configuration file './self.json' extends itself: /self.json -> /self.json",
      );
      assert_eq!(
        resolve_config("/a.json", &environment).await.unwrap_err().to_string().split("\n    at ").next().unwrap(),
        "The configuration file 'npm:config@1.0.0/dprint.json' extends itself: /a.json -> /dir/b.json -> npm:config@1.0.0/dprint.json -> npm:config@1.0.0/other.json -> npm:config@1.0.0/dprint.json",
      );

      // extending the same file in several places is fine
      let result = resolve_config("/diamond.json", &environment).await.unwrap();
      assert_eq!(get_number(&result, "prop1"), 1);
      assert_eq!(get_number(&result, "prop2"), 2);
      assert_eq!(get_number(&result, "prop3"), 3);
    });
  }

  #[test]
  fn should_resolve_exports() {
    fn target(text: &str) -> ExportResolution {
      ExportResolution::Target(text.to_string())
    }

    let text = serde_json::json!("./index.json");
    assert_eq!(resolve_export(&text, "."), target("./index.json"));
    assert_eq!(resolve_export(&text, "./sub"), ExportResolution::NotFound);

    let conditions = serde_json::json!({ "import": "./index.js", "default": "./index.json" });
    assert_eq!(resolve_export(&conditions, "."), target("./index.json"));
    assert_eq!(resolve_export(&conditions, "./sub"), ExportResolution::NotFound);

    let sub_paths = serde_json::json!({
      ".": [{ "import": "./index.js" }, "./index.json"],
      "./sub": { "dprint": { "default": "./sub.json" }, "default": "./sub.js" },
      "./none": null,
      "./condition-none": { "dprint": null, "default": "./other.json" },
    });
    assert_eq!(resolve_export(&sub_paths, "."), target("./index.json"));
    assert_eq!(resolve_export(&sub_paths, "./sub"), target("./sub.json"));
    assert_eq!(resolve_export(&sub_paths, "./none"), ExportResolution::Excluded);
    assert_eq!(resolve_export(&sub_paths, "./condition-none"), ExportResolution::Excluded);
    assert_eq!(resolve_export(&sub_paths, "./other"), ExportResolution::NotFound);

    // only uses a non-JSON file when it's for the dprint condition
    assert_eq!(resolve_export(&serde_json::json!("./index.js"), "."), ExportResolution::NotFound);
    assert_eq!(
      resolve_export(&serde_json::json!({ "import": "./index.mjs", "default": "./index.js" }), "."),
      ExportResolution::NotFound
    );
    assert_eq!(resolve_export(&serde_json::json!({ "dprint": "./config" }), "."), target("./config"));

    // subpath patterns
    let patterns = serde_json::json!({
      "./*": "./configs/*.json",
      "./langs/*": "./configs/langs/*/config.json",
      "./langs/*.json": "./configs/langs/*.json",
      "./langs/internal/*": null,
      "./langs/exact": "./exact.json",
    });
    assert_eq!(resolve_export(&patterns, "./markdown"), target("./configs/markdown.json"));
    assert_eq!(resolve_export(&patterns, "./a/b"), target("./configs/a/b.json"));
    assert_eq!(resolve_export(&patterns, "./langs/ts"), target("./configs/langs/ts/config.json"));
    assert_eq!(resolve_export(&patterns, "./langs/ts.json"), target("./configs/langs/ts.json"));
    assert_eq!(resolve_export(&patterns, "./langs/internal/ts"), ExportResolution::Excluded);
    assert_eq!(resolve_export(&patterns, "./langs/exact"), target("./exact.json"));
    assert_eq!(resolve_export(&patterns, "."), ExportResolution::NotFound);
  }

  #[test]
  fn should_join_package_paths() {
    assert_eq!(join_package_path("index.json", "./other.json").unwrap(), "other.json");
    assert_eq!(join_package_path("a/b/index.json", "../c/other.json").unwrap(), "a/c/other.json");
    assert_eq!(join_package_path("a/index.json", "b//./other.json").unwrap(), "a/b/other.json");
    assert!(join_package_path("a/index.json", "../../other.json").is_err());
    assert!(join_package_path("a/index.json", "/other.json").is_err());
    assert!(join_package_path("a/index.json", "..\\other.json").is_err());
    assert!(join_package_path("a/index.json", "C:/other.json").is_err());
  }

  #[test]
  fn cached_registry_configs_do_not_fetch_deprecation_metadata() {
    let environment = TestEnvironmentBuilder::new().build();
    add_registry_package(&environment, "cached-config", "1.0.0", &[("package/dprint.json", "{}")]);
    environment.clone().run_in_runtime(async move {
      let base = PathSource::new_local(environment.cwd());
      let metadata = "https://registry.npmjs.org/cached-config";
      let package = "npm:cached-config@1.0.0";
      NpmExtendsResolver::default().resolve(package, &base, &environment).await.unwrap();
      assert_eq!(environment.remote_file_download_count(metadata), 1);
      environment.add_unresponsive_remote_file(metadata);
      crate::utils::run_before_deadline(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        NpmExtendsResolver::default().resolve(package, &base, &environment),
      )
      .await
      .unwrap()
      .unwrap();
      assert_eq!(environment.remote_file_download_count(metadata), 1);
      environment.remove_unresponsive_remote_file(metadata);
      let resolver = NpmExtendsResolver {
        reload: true,
        ..Default::default()
      };
      resolver.resolve(package, &base, &environment).await.unwrap();
      assert_eq!(environment.remote_file_download_count(metadata), 2);
    });
  }

  #[test]
  fn warns_when_an_extended_npm_configuration_is_deprecated() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{"extends": "npm:deprecated-config@1.0.0"}"#)
      .build();
    add_registry_package(&environment, "deprecated-config", "1.0.0", &[("package/dprint.json", "{}")]);
    environment.add_remote_file_bytes(
      "https://registry.npmjs.org/deprecated-config",
      serde_json::json!({
        "versions": {"1.0.0": {"deprecated": "Use the replacement config"}}
      })
      .to_string()
      .into_bytes(),
    );
    environment.clone().run_in_runtime(async move {
      resolve_config("/dprint.json", &environment).await.unwrap();
      let messages = environment.take_stderr_messages();
      assert_eq!(messages.len(), 1);
      assert!(messages[0].contains("deprecated-config@1.0.0 is deprecated: Use the replacement config"));
    });
  }

  #[test]
  fn supports_toml_and_preserves_trusted_property_origins() {
    let environment = TestEnvironmentBuilder::new()
      .write_file("/dprint.json", r#"{ "extends": ["npm:config@1.0.0", "./trusted.toml"] }"#)
      .write_file("/trusted.toml", "[process]\nvalue = 1\n")
      .build();
    add_registry_package(&environment, "config", "1.0.0", &[("package/dprint.toml", "[process]\nvalue = 2\n")]);
    environment.clone().run_in_runtime(async move {
      let config = resolve_config("/dprint.json", &environment).await.unwrap();
      assert_eq!(get_number(&config, "process"), 2);
      let ConfigMapValue::PluginConfig(trusted) = &config.plugins.trusted_config()["process"] else {
        panic!()
      };
      assert_eq!(trusted.properties["value"], ConfigKeyValue::Number(1));
      let origin = config.plugins.trusted_origins().plugin_property("process", "value").unwrap();
      assert!(origin.local_files_allowed);
      assert_eq!(origin.source, PathSource::new_local(environment.canonicalize("/trusted.toml").unwrap()));
      assert!(!config.plugins.origins.plugin_property("process", "value").unwrap().local_files_allowed);
      environment.take_stderr_messages();
    });
  }

  #[test]
  fn registry_config_cannot_select_local_plugin_configuration() {
    for path in ["file:///private.json", "../private.json", "~/private.json", "C:\\private.json"] {
      let environment = TestEnvironmentBuilder::new()
        .write_file("/dprint.json", r#"{ "extends": "npm:config@1.0.0" }"#)
        .build();
      let config = serde_json::json!({"process": {"configFile": path}}).to_string();
      add_registry_package(&environment, "config", "1.0.0", &[("package/dprint.json", &config)]);
      environment.clone().run_in_runtime(async move {
        let error = resolve_config("/dprint.json", &environment).await.unwrap_err().to_string();
        assert!(error.contains("HTTP(S)"), "{error}");
      });
    }
  }

  #[test]
  fn refreshes_a_cached_tarball_once_when_the_pin_changes() {
    let environment = TestEnvironmentBuilder::new().build();
    add_registry_package(&environment, "config", "1.0.0", &[("package/dprint.json", "{}")]);
    environment.clone().run_in_runtime(async move {
      let base = PathSource::new_local(environment.cwd());
      let resolver = NpmExtendsResolver::default();
      resolver.resolve("npm:config@1.0.0", &base, &environment).await.unwrap();
      let checksum = add_registry_package(&environment, "config", "1.0.0", &[("package/dprint.json", "{\"lineWidth\": 90}")]);
      let resolved = resolver.resolve(&format!("npm:config@1.0.0@{checksum}"), &base, &environment).await.unwrap();
      assert_eq!(String::from_utf8(resolved.content).unwrap(), "{\"lineWidth\": 90}");
      let url = "https://registry.npmjs.org/config/-/config-1.0.0.tgz";
      assert_eq!(environment.remote_file_download_count(url), 2);
      let error = resolver
        .resolve(&format!("npm:config@1.0.0@{}", "0".repeat(64)), &base, &environment)
        .await
        .unwrap_err();
      assert!(error.to_string().contains("Invalid checksum"));
      assert_eq!(environment.remote_file_download_count(url), 3);
    });
  }

  #[test]
  fn validates_each_checksum_even_when_package_is_already_cached() {
    let environment = TestEnvironmentBuilder::new().build();
    let checksum = add_registry_package(&environment, "config", "1.0.0", &[("package/dprint.json", "{}")]);
    environment
      .write_file(
        "/dprint.json",
        &format!(r#"{{"extends": ["npm:config@1.0.0@{checksum}", "npm:config@1.0.0@{}"]}}"#, "0".repeat(64)),
      )
      .unwrap();
    environment.clone().run_in_runtime(async move {
      let error = resolve_config("/dprint.json", &environment).await.unwrap_err().to_string();
      assert!(error.contains("Invalid checksum"), "{error}");
    });
  }

  async fn resolve_config(config_path: &str, environment: &TestEnvironment) -> Result<ResolvedConfig> {
    let args = crate::options::SessionOptions {
      config: Some(crate::options::ConfigArg::PathOrUrl(config_path.to_string())),
      ..Default::default()
    };
    Ok(resolve_config_from_args(&args, environment).await?)
  }

  /// Adds a package to the test npm registry, returning its tarball's checksum.
  fn add_registry_package(environment: &TestEnvironment, name: &str, version: &str, files: &[(&str, &str)]) -> String {
    let tarball_url = format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz");
    let packument = serde_json::json!({
      "versions": {
        version: {
          "dist": { "tarball": tarball_url }
        }
      }
    });
    let files = files.iter().map(|(path, text)| (*path, text.as_bytes())).collect::<Vec<_>>();
    environment.add_remote_file_bytes(&format!("https://registry.npmjs.org/{name}"), packument.to_string().into_bytes());
    let tarball = create_test_npm_tarball(&files);
    let checksum = crate::utils::get_sha256_checksum(&tarball);
    environment.add_remote_file_bytes(&tarball_url, tarball);
    checksum
  }

  fn get_number(config: &ResolvedConfig, key: &str) -> i32 {
    match config.plugins.config.get(key) {
      Some(ConfigMapValue::PluginConfig(plugin)) => match plugin.properties.get("value") {
        Some(ConfigKeyValue::Number(value)) => *value,
        value => panic!("Expected numeric property: {value:?}"),
      },
      value => panic!("Expected a number for {}, but found {:?}", key, value),
    }
  }
}
