use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use kprint_config::plugin_config_file::CONFIG_FILE_CONTENT_PROPERTY;
use kprint_config::plugin_config_file::CONFIG_FILE_PROPERTY;
use kprint_config::plugin_config_file::ConfigFileReference;
use kprint_config::plugin_config_file::parse_config_file;
use kprint_config::plugin_config_file::select;
use kprint_configuration::ConfigKeyMap;
use kprint_configuration::ConfigKeyValue;
use kprint_plugin_types::PluginInfo;
use serde_json::Value as JsonValue;

use crate::configuration::ConfigMap;
use crate::configuration::ConfigMapValue;
use crate::configuration::RawPluginConfig;
use crate::environment::HostEnvironment;
use crate::plugins::PluginWrapper;
use kprint_config::PluginConfigFile;

pub fn get_plugin_config_map(plugin: &PluginWrapper, config_map: &mut ConfigMap) -> Result<RawPluginConfig> {
  match get_plugin_config_map_inner(plugin, config_map) {
    Ok(result) => Ok(result),
    Err(err) => bail!("Error initializing from configuration file. {:#}", err),
  }
}

fn get_plugin_config_map_inner(plugin: &PluginWrapper, config_map: &mut ConfigMap) -> Result<RawPluginConfig> {
  let config_key = &plugin.info().config_key;

  if let Some(plugin_config) = config_map.shift_remove(config_key) {
    if let ConfigMapValue::PluginConfig(plugin_config) = plugin_config {
      Ok(plugin_config)
    } else {
      bail!("Expected the configuration property '{}' to be an object.", config_key)
    }
  } else {
    Ok(Default::default())
  }
}

/// A configuration file of a plugin's own as it was when the plugin's
/// configuration was resolved, or that it didn't exist, so a long running
/// process (ex. the language server) resolves it again when it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConfigFileState {
  pub path: PathBuf,
  pub text: Option<String>,
}

impl PluginConfigFileState {
  pub fn is_current(&self, environment: &impl HostEnvironment) -> bool {
    let text = if environment.path_is_file(&self.path) {
      environment.read_file(&self.path).ok()
    } else {
      None
    };
    text == self.text
  }
}

/// Passes a plugin that reads a configuration file of its own (ex.
/// `tombi.toml`) that file, parsed: the one its `configFile` names, or else
/// the first of those it declares that exists. `"configFile": false` passes
/// none.
///
/// A plugin that declares no configuration files gets a `configFile` as a
/// property, so one with a `configFile` option of its own keeps it.
pub fn resolve_plugin_config_file(
  plugin_info: &PluginInfo,
  config_file: Option<PluginConfigFile>,
  mut properties: ConfigKeyMap,
  base_dir: &Path,
  environment: &impl HostEnvironment,
  files_read: &mut Vec<PluginConfigFileState>,
) -> Result<ConfigKeyMap> {
  let Some(default_files) = &plugin_info.config_files else {
    if let Some(config_file) = config_file {
      let value = match config_file {
        PluginConfigFile::Path(path) => ConfigKeyValue::String(path),
        PluginConfigFile::Disabled => ConfigKeyValue::Bool(false),
      };
      properties.insert(CONFIG_FILE_PROPERTY.to_string(), value);
    }
    return Ok(properties);
  };
  let read = |reference: &ConfigFileReference, files_read: &mut Vec<PluginConfigFileState>| -> Result<Option<JsonValue>> {
    let file_path = base_dir.join(&reference.path);
    if !environment.path_is_file(&file_path) {
      // one that's made later is passed then
      files_read.push(PluginConfigFileState { path: file_path, text: None });
      return Ok(None);
    }
    let text = environment
      .read_file(&file_path)
      .with_context(|| format!("Failed reading {}.", file_path.display()))?;
    files_read.push(PluginConfigFileState {
      path: file_path.clone(),
      text: Some(text.clone()),
    });
    parse_config_file(&file_path, &text).map(Some)
  };
  let found = match config_file {
    Some(PluginConfigFile::Path(text)) => {
      let reference = ConfigFileReference::parse(&text).with_context(|| format!("{}.{}", plugin_info.config_key, CONFIG_FILE_PROPERTY))?;
      let Some(content) = read(&reference, files_read)? else {
        bail!(
          "{}.{}: Could not find {}.",
          plugin_info.config_key,
          CONFIG_FILE_PROPERTY,
          base_dir.join(&reference.path).display()
        );
      };
      let part = select(&content, &reference)?.clone();
      Some((reference, part))
    }
    Some(PluginConfigFile::Disabled) => None,
    None => {
      let mut found = None;
      for default_file in default_files {
        let reference = ConfigFileReference::parse(default_file).with_context(|| format!("{} declares a configuration file of its own", plugin_info.name))?;
        // the plugin chose these, not the user, so they may only name a file
        // in the configuration's directory (a plugin from a remote
        // configuration could otherwise read any file it likes)
        if !is_plain_relative(&reference.path) {
          bail!(
            "{} declares the configuration file '{}', which isn't a path within the configuration file's directory.",
            plugin_info.name,
            reference.path
          );
        }
        if escapes_dir(base_dir, &base_dir.join(&reference.path), environment) {
          bail!(
            "{} declares the configuration file '{}', which resolves outside the configuration file's directory.",
            plugin_info.name,
            reference.path
          );
        }
        let Some(content) = read(&reference, files_read)? else {
          continue;
        };
        // a file without the part isn't the plugin's (ex. a pyproject.toml
        // without its [tool.tombi])
        if let Ok(part) = select(&content, &reference) {
          found = Some((reference, part.clone()));
          break;
        }
      }
      found
    }
  };
  let Some((reference, content)) = found else {
    return Ok(properties);
  };
  let config_path = base_dir.join(&reference.path);
  let schema_files = read_config_schemas(plugin_info, &content, &properties, &config_path, base_dir, environment, files_read)?;
  properties.insert(
    CONFIG_FILE_CONTENT_PROPERTY.to_string(),
    ConfigKeyValue::Object(ConfigKeyMap::from([
      ("path".to_string(), ConfigKeyValue::String(reference.path.clone())),
      ("content".to_string(), ConfigKeyValue::String(content.to_string())),
      ("absolutePath".to_string(), ConfigKeyValue::String(config_path.to_string_lossy().into_owned())),
      (
        "files".to_string(),
        ConfigKeyValue::Object(schema_files.into_iter().map(|(path, text)| (path, ConfigKeyValue::String(text))).collect()),
      ),
    ])),
  );
  Ok(properties)
}

fn schema_paths<'a>(value: &'a JsonValue, segments: &[String], paths: &mut Vec<&'a str>) {
  let Some((key, rest)) = segments.split_first() else {
    if let Some(path) = value.as_str() {
      paths.push(path);
    }
    return;
  };
  if key == "*" {
    if let Some(values) = value.as_array() {
      for child in values {
        schema_paths(child, rest, paths);
      }
    }
  } else if let Some(child) = value.get(key) {
    schema_paths(child, rest, paths);
  }
}

fn schema_references(value: &JsonValue, base: &url::Url, references: &mut Vec<url::Url>) -> Result<()> {
  match value {
    JsonValue::Object(properties) => {
      let base = match properties.get("$id").and_then(JsonValue::as_str) {
        Some(id) => base.join(id)?,
        None => base.clone(),
      };
      for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
        if let Some(reference) = properties.get(keyword).and_then(JsonValue::as_str) {
          references.push(base.join(reference)?);
        }
      }
      for keyword in ["$defs", "definitions", "properties", "patternProperties", "dependentSchemas", "dependencies"] {
        if let Some(children) = properties.get(keyword).and_then(JsonValue::as_object) {
          for child in children.values().filter(|child| child.is_object()) {
            schema_references(child, &base, references)?;
          }
        }
      }
      for keyword in [
        "allOf",
        "anyOf",
        "oneOf",
        "prefixItems",
        "items",
        "additionalItems",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "propertyNames",
        "contentSchema",
      ] {
        if let Some(child) = properties.get(keyword) {
          schema_references(child, &base, references)?;
        }
      }
    }
    JsonValue::Array(values) => {
      for child in values {
        schema_references(child, base, references)?;
      }
    }
    _ => {}
  }
  Ok(())
}

fn read_config_schemas(
  plugin_info: &PluginInfo,
  content: &JsonValue,
  properties: &ConfigKeyMap,
  config_path: &Path,
  base_dir: &Path,
  environment: &impl HostEnvironment,
  files_read: &mut Vec<PluginConfigFileState>,
) -> Result<std::collections::BTreeMap<String, String>> {
  let mut files = std::collections::BTreeMap::new();
  if plugin_info.config_schema_paths.is_empty() {
    return Ok(files);
  }
  let mut selected = content.clone();
  if let Some(object) = selected.as_object_mut() {
    for (key, value) in properties {
      object.insert(key.clone(), serde_json::to_value(value)?);
    }
  }
  let config_uri = url::Url::from_file_path(config_path).map_err(|()| anyhow::anyhow!("Invalid configuration path: {}", config_path.display()))?;
  let mut paths = Vec::new();
  for segments in &plugin_info.config_schema_paths {
    schema_paths(&selected, segments, &mut paths);
  }
  let mut pending = paths
    .into_iter()
    .map(|path| config_uri.join(path))
    .collect::<std::result::Result<Vec<_>, _>>()?;
  while let Some(mut uri) = pending.pop() {
    if uri.scheme() != "file" {
      continue;
    }
    uri.set_fragment(None);
    let path = uri.to_file_path().map_err(|()| anyhow::anyhow!("Invalid schema file URI: {uri}"))?;
    let key = path.to_string_lossy().into_owned();
    if files.contains_key(&key) {
      continue;
    }
    if !path.starts_with(base_dir) || escapes_dir(base_dir, &path, environment) {
      bail!("Schema {} resolves outside {}.", path.display(), base_dir.display());
    }
    let text = environment
      .read_file(&path)
      .with_context(|| format!("Failed reading schema {}.", path.display()))?;
    let schema: JsonValue = serde_json::from_str(&text).with_context(|| format!("Invalid JSON Schema in {}.", path.display()))?;
    schema_references(&schema, &uri, &mut pending)?;
    files_read.push(PluginConfigFileState {
      path,
      text: Some(text.clone()),
    });
    files.insert(key, text);
  }
  Ok(files)
}

/// Whether a path only names directories and a file below where it's
/// joined to: not absolute, and without a `..`.
fn is_plain_relative(path: &str) -> bool {
  Path::new(path)
    .components()
    .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Whether an existing file is outside a directory once symlinks are
/// followed. One that doesn't exist (yet) isn't.
fn escapes_dir(dir: &Path, file_path: &Path, environment: &impl HostEnvironment) -> bool {
  if !environment.path_is_file(file_path) {
    return false;
  }
  match (environment.canonicalize(dir), environment.canonicalize(file_path)) {
    (Ok(dir), Ok(file_path)) => !file_path.starts_with(&dir),
    // what can't be resolved can't be shown to be inside
    _ => true,
  }
}

#[cfg(test)]
mod tests {
  use crate::plugins::TestPlugin;
  use kprint_configuration::ConfigKeyMap;
  use kprint_configuration::ConfigKeyValue;

  use super::*;
  use crate::configuration::ConfigMap;
  use crate::configuration::ConfigMapValue;

  #[test]
  fn should_get_config_for_plugin() {
    let mut config_map = ConfigMap::new();
    let ts_plugin = RawPluginConfig {
      associations: None,
      embedded: None,
      locked: false,
      overrides: Vec::new(),
      properties: ConfigKeyMap::from([("lineWidth".to_string(), ConfigKeyValue::from_i32(40))]),
      config_file: None,
    };
    config_map.insert(String::from("lineWidth"), ConfigMapValue::from_i32(80));
    config_map.insert(String::from("typescript"), ConfigMapValue::PluginConfig(ts_plugin.clone()));
    let plugin = PluginWrapper::new(Box::new(create_plugin()));
    let result = get_plugin_config_map(&plugin, &mut config_map).unwrap();
    assert_eq!(result, ts_plugin);
    assert!(!config_map.contains_key("typescript"));
  }

  #[test]
  fn should_error_plugin_key_is_not_object() {
    let mut config_map = ConfigMap::new();
    config_map.insert(String::from("lineWidth"), ConfigMapValue::from_i32(80));
    config_map.insert(String::from("typescript"), ConfigMapValue::from(""));
    assert_errors(&mut config_map, "Expected the configuration property 'typescript' to be an object.");
  }

  fn assert_errors(config_map: &mut ConfigMap, message: &str) {
    let plugin = PluginWrapper::new(Box::new(create_plugin()));
    let result = get_plugin_config_map(&plugin, config_map);
    assert_eq!(
      result.err().unwrap().to_string(),
      format!("Error initializing from configuration file. {}", message)
    );
  }

  fn tombi_info(config_files: Option<Vec<&str>>) -> PluginInfo {
    let mut info = PluginWrapper::new(Box::new(TestPlugin::new("dprint-plugin-tombi", "toml", vec![".toml"], vec![])))
      .info()
      .clone();
    info.config_files = config_files.map(|files| files.into_iter().map(String::from).collect());
    info
  }

  fn config_file_content(properties: &ConfigKeyMap) -> Option<(String, serde_json::Value)> {
    let ConfigKeyValue::Object(object) = properties.get(CONFIG_FILE_CONTENT_PROPERTY)? else {
      panic!("not an object");
    };
    let path = object.get("path").and_then(|value| value.as_string()).unwrap().clone();
    let content = serde_json::from_str(object.get("content").and_then(|value| value.as_string()).unwrap()).unwrap();
    Some((path, content))
  }

  fn write(environment: &crate::environment::TestEnvironment, path: &str, text: &str) {
    use kprint_platform::environment::*;
    environment.mk_dir_all(Path::new(path).parent().unwrap()).unwrap();
    environment.write_file(path, text).unwrap();
  }

  fn properties(values: &[(&str, ConfigKeyValue)]) -> ConfigKeyMap {
    values.iter().map(|(key, value)| (key.to_string(), value.clone())).collect()
  }

  #[test]
  fn passes_the_first_config_file_the_plugin_declares() {
    let environment = crate::environment::TestEnvironment::new();
    write(
      &environment,
      "/project/.config/tombi.toml",
      "[format.rules]
line-width = 140
",
    );
    let info = tombi_info(Some(vec!["tombi.toml", ".config/tombi.toml"]));
    let result = resolve_plugin_config_file(&info, None, properties(&[]), Path::new("/project"), &environment, &mut Vec::new()).unwrap();
    assert_eq!(
      config_file_content(&result),
      Some((
        ".config/tombi.toml".to_string(),
        serde_json::json!({ "format": { "rules": { "line-width": 140 } } })
      ))
    );
    // none exists
    let environment = crate::environment::TestEnvironment::new();
    let result = resolve_plugin_config_file(&info, None, properties(&[]), Path::new("/project"), &environment, &mut Vec::new()).unwrap();
    assert_eq!(result, properties(&[]));
  }

  #[test]
  fn passes_the_part_of_the_config_file_it_names() {
    let environment = crate::environment::TestEnvironment::new();
    write(
      &environment,
      "/project/tombi.toml",
      "line-width = 1
",
    );
    write(
      &environment,
      "/project/pyproject.toml",
      "[tool.tombi.format.rules]
line-width = 100
",
    );
    let info = tombi_info(Some(vec!["tombi.toml"]));
    let result = resolve_plugin_config_file(
      &info,
      Some(PluginConfigFile::Path("pyproject.toml#tool.tombi".to_string())),
      properties(&[("lineWidth", ConfigKeyValue::from_i32(80))]),
      Path::new("/project"),
      &environment,
      &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
      config_file_content(&result),
      Some((
        "pyproject.toml".to_string(),
        serde_json::json!({ "format": { "rules": { "line-width": 100 } } })
      ))
    );
    // the plugin's own properties are passed with it
    assert_eq!(result.get("lineWidth"), Some(&ConfigKeyValue::from_i32(80)));

    // false passes none
    let result = resolve_plugin_config_file(
      &info,
      Some(PluginConfigFile::Disabled),
      properties(&[]),
      Path::new("/project"),
      &environment,
      &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(result, properties(&[]));
  }

  #[test]
  fn errors_for_a_config_file_it_cant_pass() {
    let environment = crate::environment::TestEnvironment::new();
    write(&environment, "/project/tombi.toml", "a = ");
    let info = tombi_info(Some(vec!["tombi.toml"]));
    let error = |path: &str| {
      resolve_plugin_config_file(
        &info,
        Some(PluginConfigFile::Path(path.to_string())),
        properties(&[]),
        Path::new("/project"),
        &environment,
        &mut Vec::new(),
      )
      .unwrap_err()
      .to_string()
    };
    assert_eq!(
      error("missing.toml"),
      format!("toml.configFile: Could not find {}.", Path::new("/project").join("missing.toml").display())
    );
    assert!(error("tombi.toml").starts_with("Failed parsing"));
    assert!(error("tombi.toml#").starts_with("toml.configFile"));
  }

  #[test]
  fn skips_a_declared_config_file_without_the_part_it_names() {
    let environment = crate::environment::TestEnvironment::new();
    write(
      &environment,
      "/project/pyproject.toml",
      "[project]
name = \"x\"
",
    );
    let info = tombi_info(Some(vec!["pyproject.toml#tool.tombi"]));
    let mut files_read = Vec::new();
    let result = resolve_plugin_config_file(&info, None, properties(&[]), Path::new("/project"), &environment, &mut files_read).unwrap();
    assert_eq!(result, properties(&[]));
    // until it has it
    assert!(files_read[0].is_current(&environment));
    write(
      &environment,
      "/project/pyproject.toml",
      "[tool.tombi.format.rules]
line-width = 100
",
    );
    assert!(!files_read[0].is_current(&environment));
    let result = resolve_plugin_config_file(&info, None, properties(&[]), Path::new("/project"), &environment, &mut Vec::new()).unwrap();
    assert_eq!(
      config_file_content(&result),
      Some((
        "pyproject.toml".to_string(),
        serde_json::json!({ "format": { "rules": { "line-width": 100 } } })
      ))
    );
  }

  #[test]
  fn records_the_config_files_it_looked_at_until_they_change() {
    use kprint_platform::environment::*;
    let environment = crate::environment::TestEnvironment::new();
    write(
      &environment,
      "/project/.config/tombi.toml",
      "line-width = 1
",
    );
    let info = tombi_info(Some(vec!["tombi.toml", ".config/tombi.toml", ".tombi.toml"]));
    let mut files_read = Vec::new();
    resolve_plugin_config_file(&info, None, properties(&[]), Path::new("/project"), &environment, &mut files_read).unwrap();
    // the one before it that didn't exist, and it; not the ones after it
    assert_eq!(
      files_read,
      vec![
        PluginConfigFileState {
          path: Path::new("/project").join("tombi.toml"),
          text: None,
        },
        PluginConfigFileState {
          path: Path::new("/project").join(".config/tombi.toml"),
          text: Some(
            "line-width = 1
"
            .to_string()
          ),
        },
      ]
    );
    assert!(files_read.iter().all(|file| file.is_current(&environment)));

    // changing it
    write(
      &environment,
      "/project/.config/tombi.toml",
      "line-width = 2
",
    );
    assert!(!files_read[1].is_current(&environment));
    // or making one before it
    write(&environment, "/project/tombi.toml", "");
    assert!(!files_read[0].is_current(&environment));
    // or removing it
    environment.remove_file("/project/.config/tombi.toml").unwrap();
    assert!(!files_read[1].is_current(&environment));
  }

  #[test]
  fn errors_for_a_declared_config_file_outside_the_configs_directory() {
    let environment = crate::environment::TestEnvironment::new();
    write(&environment, "/secret.json", "{ \"token\": \"x\" }");
    write(&environment, "/project/tombi.toml", "");
    for path in ["../secret.json", "/secret.json", "sub/../../secret.json"] {
      let mut files_read = Vec::new();
      let error = resolve_plugin_config_file(
        &tombi_info(Some(vec![path])),
        None,
        properties(&[]),
        Path::new("/project"),
        &environment,
        &mut files_read,
      )
      .unwrap_err()
      .to_string();
      assert_eq!(
        error,
        format!(
          "dprint-plugin-tombi declares the configuration file '{}', which isn't a path within the configuration file's directory.",
          path
        )
      );
      // nothing was read
      assert_eq!(files_read, Vec::new());
    }
    // a file the user names is theirs to choose
    let result = resolve_plugin_config_file(
      &tombi_info(Some(vec!["tombi.toml"])),
      Some(PluginConfigFile::Path("../secret.json".to_string())),
      properties(&[]),
      Path::new("/project"),
      &environment,
      &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
      config_file_content(&result),
      Some(("../secret.json".to_string(), serde_json::json!({ "token": "x" })))
    );
  }

  #[test]
  fn passes_a_plugin_without_config_files_its_config_file_as_a_property() {
    let environment = crate::environment::TestEnvironment::new();
    // ex. a plugin with a configFile option of its own
    let result = resolve_plugin_config_file(
      &tombi_info(None),
      Some(PluginConfigFile::Path("own.json".to_string())),
      properties(&[]),
      Path::new("/project"),
      &environment,
      &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(result, properties(&[(CONFIG_FILE_PROPERTY, ConfigKeyValue::from_str("own.json"))]));
    let result = resolve_plugin_config_file(&tombi_info(None), None, properties(&[]), Path::new("/project"), &environment, &mut Vec::new()).unwrap();
    assert_eq!(result, properties(&[]));
  }

  fn create_plugin() -> TestPlugin {
    TestPlugin::new("dprint-plugin-typescript", "typescript", vec![".ts"], vec![])
  }

  #[test]
  fn passes_local_schemas_and_references_and_detects_changes() {
    let environment = crate::environment::TestEnvironment::new();
    let root = if cfg!(windows) { "C:/project" } else { "/project" };
    write(
      &environment,
      &format!("{root}/config/tombi.toml"),
      "[[schemas]]\npath = 'schema.json'\ninclude = ['example.toml']\n",
    );
    write(
      &environment,
      &format!("{root}/config/schema.json"),
      r#"{"$ref":"parts/fields.json#/$defs/fields"}"#,
    );
    write(&environment, &format!("{root}/config/parts/fields.json"), r#"{"$ref":"../schema.json"}"#);
    let mut info = tombi_info(Some(vec!["config/tombi.toml"]));
    info.config_schema_paths = vec![vec!["schemas".into(), "*".into(), "path".into()]];
    let mut files_read = Vec::new();
    let result = resolve_plugin_config_file(&info, None, properties(&[]), Path::new(root), &environment, &mut files_read).unwrap();
    let ConfigKeyValue::Object(config) = &result[CONFIG_FILE_CONTENT_PROPERTY] else {
      panic!("missing config");
    };
    let ConfigKeyValue::Object(files) = &config["files"] else {
      panic!("missing schema files");
    };
    assert_eq!(files.len(), 2);
    assert_eq!(files_read.len(), 3);
    assert!(files_read.iter().all(|file| file.is_current(&environment)));
    write(&environment, &format!("{root}/config/parts/fields.json"), "{}");
    assert!(!files_read.iter().all(|file| file.is_current(&environment)));
  }

  #[test]
  fn rejects_missing_invalid_and_outside_schema_files() {
    for (schema_path, schema_content, expected) in [
      ("missing.json", None, "Failed reading schema"),
      ("schema.json", Some("not JSON"), "Invalid JSON Schema"),
      ("../secret.json", Some("{}"), "resolves outside"),
    ] {
      let environment = crate::environment::TestEnvironment::new();
      let root = if cfg!(windows) { "C:/project" } else { "/project" };
      write(
        &environment,
        &format!("{root}/tombi.toml"),
        &format!("[[schemas]]\npath = '{schema_path}'\ninclude = ['example.toml']\n"),
      );
      if let Some(content) = schema_content {
        write(&environment, &format!("{root}/schema.json"), content);
      }
      let mut info = tombi_info(Some(vec!["tombi.toml"]));
      info.config_schema_paths = vec![vec!["schemas".into(), "*".into(), "path".into()]];
      let error = resolve_plugin_config_file(&info, None, properties(&[]), Path::new(root), &environment, &mut Vec::new()).unwrap_err();
      assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
  }

  #[test]
  fn follows_schema_references_without_reading_example_values() {
    let schema = serde_json::json!({
      "$id": "nested/root.json",
      "properties": {"setting": {"$ref": "setting.json"}},
      "$defs": {"entry": {"items": {"$ref": "entry.json"}}},
      "examples": [{"$ref": "not-a-schema.json"}],
      "default": {"$ref": "also-not-a-schema.json"}
    });
    let base = url::Url::parse("file:///project/schema.json").unwrap();
    let mut references = Vec::new();
    schema_references(&schema, &base, &mut references).unwrap();
    let mut references = references.iter().map(url::Url::as_str).collect::<Vec<_>>();
    references.sort_unstable();
    assert_eq!(references, ["file:///project/nested/entry.json", "file:///project/nested/setting.json"]);
  }
}
