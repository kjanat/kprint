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
pub enum PluginConfigFileState {
  Missing { path: PathBuf },
  Local { path: PathBuf, text: String },
  Confined { directory: PathBuf, path: PathBuf, text: String },
  Remote { expires_at: u64 },
}

impl PluginConfigFileState {
  pub fn is_current(&self, environment: &impl HostEnvironment) -> bool {
    match self {
      Self::Missing { path } => !environment.path_is_file(path),
      Self::Confined { directory, path, text } => environment.read_file_within(directory, path).is_ok_and(|current| current == *text),
      Self::Remote { expires_at } => environment.get_time_secs() < *expires_at,
      Self::Local { path, text } => environment.read_file(path).is_ok_and(|current| current == *text),
    }
  }
}

/// Passes a plugin that reads a configuration file of its own (ex.
/// `tombi.toml`) that file, parsed: the one its `configFile` names, or else
/// the first of those it declares that exists. `"configFile": false` passes
/// none.
///
/// A plugin that declares no configuration files gets a `configFile` as a
/// property, so one with a `configFile` option of its own keeps it.
pub async fn resolve_plugin_config_file(
  plugin_info: &PluginInfo,
  mut config_file: Option<PluginConfigFile>,
  mut properties: ConfigKeyMap,
  base_dir: &Path,
  environment: &impl HostEnvironment,
  files_read: &mut Vec<PluginConfigFileState>,
) -> Result<ConfigKeyMap> {
  if plugin_info.config_files.is_none() {
    if let Some(config_file) = config_file.take() {
      let value = match config_file {
        PluginConfigFile::Path(path) => ConfigKeyValue::String(path),
        PluginConfigFile::RemotePath { path, .. } => ConfigKeyValue::String(path),
        PluginConfigFile::Disabled => ConfigKeyValue::Bool(false),
      };
      properties.insert(CONFIG_FILE_PROPERTY.to_string(), value);
    }
    if plugin_info.config_schema_paths.is_empty() {
      return Ok(properties);
    }
  }
  let default_files = plugin_info.config_files.as_deref().unwrap_or_default();
  let found = match config_file {
    Some(PluginConfigFile::Path(text) | PluginConfigFile::RemotePath { url: text, .. }) => {
      let reference = ConfigFileReference::parse(&text).with_context(|| format!("{}.{}", plugin_info.config_key, CONFIG_FILE_PROPERTY))?;
      let Some((content, source, text)) = read_plugin_config(&reference, base_dir, ConfigFileAccess::UserSelected, environment, files_read).await? else {
        bail!(
          "{}.{}: Could not find {}.",
          plugin_info.config_key,
          CONFIG_FILE_PROPERTY,
          base_dir.join(&reference.path).display()
        );
      };
      let part = select(&content, &reference)?.clone();
      Some((reference, part, source, text))
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
        let Some((content, source, text)) = read_plugin_config(&reference, base_dir, ConfigFileAccess::Confined, environment, files_read).await? else {
          continue;
        };
        // a file without the part isn't the plugin's (ex. a pyproject.toml
        // without its [tool.tombi])
        if let Ok(part) = select(&content, &reference) {
          found = Some((reference, part.clone(), source, text));
          break;
        }
      }
      found
    }
  };
  let (reference, content, source, text) = match found {
    Some(found) => found,
    None => {
      let content = serde_json::json!({});
      let path = base_dir.join("config.json");
      let source = file_url(&path)?;
      let mut paths = Vec::new();
      let inline = serde_json::to_value(&properties)?;
      for segments in &plugin_info.config_schema_paths {
        schema_paths(&inline, segments, &mut paths);
      }
      if paths.is_empty() {
        return Ok(properties);
      }
      (ConfigFileReference::parse("config.json")?, content, source, String::new())
    }
  };
  let config_path = base_dir.join(&reference.path);
  let mut schemas = SchemaResources::default();
  let mut schema_content = content.clone();
  if let Some(object) = schema_content.as_object_mut() {
    for (key, value) in &properties {
      object.insert(key.clone(), serde_json::to_value(value)?);
    }
  }
  read_config_schemas(plugin_info, &schema_content, &source, base_dir, environment, files_read, &mut schemas).await?;
  let documents = read_config_references(plugin_info, &content, &source, base_dir, environment, files_read).await?;
  for document in documents.values() {
    let content = parse_config_file(Path::new(document.source.path()), &document.text)?;
    let schema_base = if plugin_info.config_local_schemas_use_root && document.source.scheme() == "file" {
      &source
    } else {
      &document.source
    };
    read_config_schemas(plugin_info, &content, schema_base, base_dir, environment, files_read, &mut schemas).await?;
  }
  properties.insert(
    CONFIG_FILE_CONTENT_PROPERTY.to_string(),
    ConfigKeyValue::Object(ConfigKeyMap::from([
      ("path".to_string(), ConfigKeyValue::String(reference.path.clone())),
      ("content".to_string(), ConfigKeyValue::String(content.to_string())),
      ("source".to_string(), ConfigKeyValue::String(source.to_string())),
      ("baseDirectory".to_string(), ConfigKeyValue::String(base_dir.to_string_lossy().into_owned())),
      (
        "documents".to_string(),
        ConfigKeyValue::Object(
          documents
            .into_iter()
            .map(|(uri, document)| {
              (
                uri,
                ConfigKeyValue::Object(ConfigKeyMap::from([
                  ("source".into(), ConfigKeyValue::String(document.source.to_string())),
                  ("text".into(), ConfigKeyValue::String(document.text)),
                ])),
              )
            })
            .collect(),
        ),
      ),
      ("text".to_string(), ConfigKeyValue::String(text)),
      (
        "fileSources".to_string(),
        ConfigKeyValue::Object(schemas.sources.into_iter().map(|(url, source)| (url, ConfigKeyValue::String(source))).collect()),
      ),
      (
        "absolutePath".to_string(),
        if source.scheme() == "file" {
          ConfigKeyValue::String(config_path.to_string_lossy().into_owned())
        } else {
          ConfigKeyValue::Null
        },
      ),
      (
        "files".to_string(),
        ConfigKeyValue::Object(schemas.files.into_iter().map(|(path, text)| (path, ConfigKeyValue::String(text))).collect()),
      ),
    ])),
  );
  Ok(properties)
}

enum ConfigFileAccess {
  UserSelected,
  Confined,
}

async fn read_plugin_config(
  reference: &ConfigFileReference,
  base_dir: &Path,
  access: ConfigFileAccess,
  environment: &impl HostEnvironment,
  files_read: &mut Vec<PluginConfigFileState>,
) -> Result<Option<(JsonValue, url::Url, String)>> {
  if let Ok(uri) = url::Url::parse(&reference.path)
    && matches!(uri.scheme(), "http" | "https")
  {
    let resolved = kprint_platform::utils::resolve_path_source_to_file_with_cache(kprint_platform::utils::PathSource::new_remote(uri), environment)
      .await?
      .into_text()?;
    let kprint_platform::utils::PathSource::Remote(remote) = resolved.source else {
      unreachable!()
    };
    files_read.push(PluginConfigFileState::Remote {
      expires_at: environment.get_time_secs() + kprint_platform::utils::REMOTE_FILE_MAX_AGE_SECS,
    });
    let content = parse_config_file(Path::new(remote.url.path()), &resolved.content)?;
    return Ok(Some((content, remote.url, resolved.content)));
  }
  let path = base_dir.join(&reference.path);
  if !environment.path_is_file(&path) {
    files_read.push(PluginConfigFileState::Missing { path });
    return Ok(None);
  }
  let text = match access {
    ConfigFileAccess::UserSelected => environment.read_file(&path),
    ConfigFileAccess::Confined => environment.read_file_within(base_dir, &path),
  }
  .with_context(|| format!("Failed reading {}.", path.display()))?;
  files_read.push(match access {
    ConfigFileAccess::UserSelected => PluginConfigFileState::Local {
      path: path.clone(),
      text: text.clone(),
    },
    ConfigFileAccess::Confined => PluginConfigFileState::Confined {
      directory: base_dir.to_owned(),
      path: path.clone(),
      text: text.clone(),
    },
  });
  let source = file_url(&path)?;
  Ok(Some((parse_config_file(&path, &text)?, source, text)))
}

fn file_url(path: &Path) -> Result<url::Url> {
  url::Url::from_file_path(path)
    .or_else(|()| {
      if !path.has_root() {
        return Err(());
      }
      let mut uri = url::Url::parse("file:///").expect("file URL is valid");
      uri.set_path(&path.to_string_lossy().replace('\\', "/"));
      Ok(uri)
    })
    .map_err(|()| anyhow::anyhow!("Invalid configuration path: {}", path.display()))
}

#[derive(Clone)]
struct ConfigDocument {
  source: url::Url,
  text: String,
}

async fn read_config_references(
  plugin: &PluginInfo,
  content: &JsonValue,
  source: &url::Url,
  base_dir: &Path,
  environment: &impl HostEnvironment,
  files_read: &mut Vec<PluginConfigFileState>,
) -> Result<std::collections::BTreeMap<String, ConfigDocument>> {
  let mut documents = std::collections::BTreeMap::new();
  let mut file_count = 0;
  let mut pending = vec![(source.clone(), content.clone(), 0)];
  while let Some((base, content, depth)) = pending.pop() {
    let mut paths = Vec::new();
    for segments in &plugin.config_reference_paths {
      schema_paths(&content, segments, &mut paths);
    }
    for path in paths {
      let uri = config_reference_base(plugin, &base)?.join(path)?;
      if documents.contains_key(uri.as_str()) {
        continue;
      }
      if file_count >= 256 || depth >= 32 {
        bail!("Plugin configuration references exceed 256 files or 32 levels at {uri}.");
      }
      let reference = match uri.scheme() {
        "http" | "https" => ConfigFileReference::parse(uri.as_str())?,
        "file" if base.scheme() == "file" => {
          let path = uri.to_file_path().map_err(|()| anyhow::anyhow!("Invalid configuration URI: {uri}"))?;
          if !path.starts_with(base_dir) || escapes_dir(base_dir, &path, environment) {
            bail!("Referenced configuration {uri} is outside {}.", base_dir.display());
          }
          ConfigFileReference::parse(&path.to_string_lossy())?
        }
        _ => bail!("Configuration {base} cannot reference {uri}. Remote configurations may only reference HTTP(S) files."),
      };
      let Some((content, resolved, text)) = read_plugin_config(&reference, base_dir, ConfigFileAccess::Confined, environment, files_read).await? else {
        bail!("Could not read referenced configuration {uri}.");
      };
      let document = ConfigDocument {
        source: resolved.clone(),
        text,
      };
      documents.insert(uri.to_string(), document.clone());
      documents.insert(resolved.to_string(), document);
      file_count += 1;
      pending.push((resolved, content, depth + 1));
    }
  }
  Ok(documents)
}

fn config_reference_base(plugin: &PluginInfo, source: &url::Url) -> Result<url::Url> {
  if source.scheme() == "file" {
    let path = source.to_file_path().map_err(|()| anyhow::anyhow!("Invalid configuration URI: {source}"))?;
    if let Some((_, directory)) = plugin
      .config_relative_path_bases
      .iter()
      .filter(|(suffix, _)| !suffix.is_empty() && path.ends_with(suffix))
      .max_by_key(|(suffix, _)| suffix.len())
    {
      if directory.contains(':') || directory.contains('\\') || directory.starts_with('/') {
        bail!("Plugin configuration path bases must be relative directories: {directory}");
      }
      return Ok(source.join(&format!("{}/", directory.trim_end_matches('/')))?);
    }
  }
  Ok(source.clone())
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

fn schema_references(value: &JsonValue, base: &url::Url, references: &mut Vec<url::Url>, resources: &mut std::collections::BTreeSet<String>) -> Result<()> {
  match value {
    JsonValue::Object(properties) => {
      let base = match properties.get("$id").and_then(JsonValue::as_str) {
        Some(id) => base.join(id)?,
        None => base.clone(),
      };
      if properties.contains_key("$id") {
        let mut resource = base.clone();
        resource.set_fragment(None);
        resources.insert(resource.to_string());
      }
      for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
        if let Some(reference) = properties.get(keyword).and_then(JsonValue::as_str) {
          references.push(base.join(reference)?);
        }
      }
      for keyword in ["$defs", "definitions", "properties", "patternProperties", "dependentSchemas", "dependencies"] {
        if let Some(children) = properties.get(keyword).and_then(JsonValue::as_object) {
          for child in children.values().filter(|child| child.is_object()) {
            schema_references(child, &base, references, resources)?;
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
          schema_references(child, &base, references, resources)?;
        }
      }
    }
    JsonValue::Array(values) => {
      for child in values {
        schema_references(child, base, references, resources)?;
      }
    }
    _ => {}
  }
  Ok(())
}

#[derive(Default)]
struct SchemaResources {
  files: std::collections::BTreeMap<String, String>,
  sources: std::collections::BTreeMap<String, String>,
  identifiers: std::collections::BTreeSet<String>,
  count: usize,
}

async fn read_config_schemas(
  plugin_info: &PluginInfo,
  content: &JsonValue,
  config_uri: &url::Url,
  base_dir: &Path,
  environment: &impl HostEnvironment,
  files_read: &mut Vec<PluginConfigFileState>,
  schemas: &mut SchemaResources,
) -> Result<()> {
  if plugin_info.config_schema_paths.is_empty() {
    return Ok(());
  }
  let mut paths = Vec::new();
  for segments in &plugin_info.config_schema_paths {
    schema_paths(content, segments, &mut paths);
  }
  let mut pending = paths
    .into_iter()
    .map(|path| {
      if config_uri.scheme() == "file" && Path::new(path).is_absolute() {
        file_url(Path::new(path))
      } else {
        config_reference_base(plugin_info, config_uri)?.join(path).map_err(Into::into)
      }
    })
    .collect::<Result<Vec<_>>>()?;
  while let Some(mut uri) = pending.pop() {
    uri.set_fragment(None);
    if schemas.identifiers.contains(uri.as_str()) {
      continue;
    }
    if matches!(uri.scheme(), "http" | "https") {
      if schemas.files.contains_key(uri.as_str()) {
        continue;
      }
      if schemas.count >= 256 {
        bail!("Schema references exceed 256 files at {uri}.");
      }
      let resolved = kprint_platform::utils::resolve_path_source_to_file_with_cache(kprint_platform::utils::PathSource::new_remote(uri.clone()), environment)
        .await?
        .into_text()?;
      let kprint_platform::utils::PathSource::Remote(remote) = resolved.source else {
        unreachable!()
      };
      let schema: JsonValue = serde_json::from_str(&resolved.content).with_context(|| format!("Invalid JSON Schema in {uri}."))?;
      let mut references = Vec::new();
      schema_references(&schema, &remote.url, &mut references, &mut schemas.identifiers)?;
      if references.iter().any(|uri| !matches!(uri.scheme(), "http" | "https")) {
        bail!("Remote schema {uri} may only reference HTTP(S) schemas.");
      }
      pending.extend(references);
      files_read.push(PluginConfigFileState::Remote {
        expires_at: environment.get_time_secs() + kprint_platform::utils::REMOTE_FILE_MAX_AGE_SECS,
      });
      schemas.count += 1;
      schemas.sources.insert(uri.to_string(), remote.url.to_string());
      schemas.files.insert(uri.to_string(), resolved.content.clone());
      schemas.files.insert(remote.url.to_string(), resolved.content);
      continue;
    }
    if uri.scheme() != "file" {
      bail!("Unsupported schema URL: {uri}");
    }
    if config_uri.scheme() != "file" {
      bail!("Remote configuration {config_uri} cannot read local schema {uri}.");
    }
    let path = uri.to_file_path().map_err(|()| anyhow::anyhow!("Invalid schema file URI: {uri}"))?;
    let key = path.to_string_lossy().into_owned();
    if schemas.files.contains_key(&key) {
      continue;
    }
    if schemas.count >= 256 {
      bail!("Schema references exceed 256 files at {uri}.");
    }
    if !path.starts_with(base_dir) || escapes_dir(base_dir, &path, environment) {
      bail!("Schema {} resolves outside {}.", path.display(), base_dir.display());
    }
    if !environment.path_is_file(&path) {
      bail!("Failed reading schema {}: not a regular file.", path.display());
    }
    let text = environment
      .read_file_within(base_dir, &path)
      .with_context(|| format!("Failed reading schema {}.", path.display()))?;
    let schema: JsonValue = serde_json::from_str(&text).with_context(|| format!("Invalid JSON Schema in {}.", path.display()))?;
    schema_references(&schema, &uri, &mut pending, &mut schemas.identifiers)?;
    files_read.push(PluginConfigFileState::Confined {
      directory: base_dir.to_owned(),
      path,
      text: text.clone(),
    });
    schemas.count += 1;
    schemas.files.insert(key, text);
  }
  Ok(())
}

/// Whether a path only names directories and a file below where it's
/// joined to: not absolute, and without a `..`.
fn is_plain_relative(path: &str) -> bool {
  !path.contains(':')
    && !path.contains('\\')
    && Path::new(path)
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

  fn resolve_plugin_config_file(
    info: &PluginInfo,
    file: Option<PluginConfigFile>,
    properties: ConfigKeyMap,
    base: &Path,
    environment: &crate::environment::TestEnvironment,
    files: &mut Vec<PluginConfigFileState>,
  ) -> Result<ConfigKeyMap> {
    environment.run_in_runtime(super::resolve_plugin_config_file(info, file, properties, base, environment, files))
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
        PluginConfigFileState::Missing {
          path: Path::new("/project").join("tombi.toml"),
        },
        PluginConfigFileState::Confined {
          directory: PathBuf::from("/project"),
          path: Path::new("/project").join(".config/tombi.toml"),
          text: "line-width = 1
"
          .to_string(),
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
    for path in [
      "../secret.json",
      "/secret.json",
      "sub/../../secret.json",
      "https://127.0.0.1/tokens.json",
      "http://localhost/tokens.json",
    ] {
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
    let result = resolve_plugin_config_file(
      &tombi_info(None),
      Some(PluginConfigFile::RemotePath {
        path: "own.json".into(),
        url: "https://example.com/own.json".into(),
      }),
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

  #[test]
  fn redirected_configs_count_once_towards_the_file_limit() {
    for count in [256, 257] {
      let environment = crate::environment::TestEnvironment::new();
      let mut paths = Vec::new();
      for index in 0..count {
        let path = format!("{index}.json");
        let target = format!("https://cdn.example.com/{path}");
        environment.add_remote_file_redirect(&format!("https://example.com/{path}"), &target);
        environment.add_remote_file(&target, b"{}");
        paths.push(path);
      }
      let content = serde_json::json!({"extends": paths});
      let mut info = tombi_info(Some(vec![]));
      info.config_reference_paths = vec![vec!["extends".into(), "*".into()]];
      let result = environment.run_in_runtime(super::read_config_references(
        &info,
        &content,
        &url::Url::parse("https://example.com/root.json").unwrap(),
        Path::new("/project"),
        &environment,
        &mut Vec::new(),
      ));
      if count == 256 {
        assert_eq!(result.unwrap().len(), 512);
      } else {
        assert!(result.err().unwrap().to_string().contains("exceed 256 files"));
      }
    }
  }

  #[test]
  fn downloads_remote_config_references_and_schemas_once() {
    let environment = crate::environment::TestEnvironment::new();
    environment.set_fs_time(1000);
    environment.add_remote_file_redirect("https://example.com/tombi.toml", "https://cdn.example.com/team/tombi.toml");
    environment.add_remote_file(
      "https://cdn.example.com/team/tombi.toml",
      b"extends = ['../base/tombi.toml']\n[[schemas]]\npath = 'schema.json'\ninclude = ['*.toml']",
    );
    environment.add_remote_file_redirect("https://cdn.example.com/base/tombi.toml", "https://cdn.example.com/final/tombi.toml");
    environment.add_remote_file("https://cdn.example.com/final/tombi.toml", b"[format.rules]\nline-width = 120");
    environment.add_remote_file("https://cdn.example.com/team/schema.json", br#"{"$ref":"parts/value.json"}"#);
    environment.add_remote_file("https://cdn.example.com/team/parts/value.json", br#"{"type":"string"}"#);
    let mut info = tombi_info(Some(vec!["tombi.toml"]));
    info.config_reference_paths = vec![vec!["extends".into(), "*".into()]];
    info.config_schema_paths = vec![vec!["schemas".into(), "*".into(), "path".into()]];
    let mut states = Vec::new();
    for _ in 0..2 {
      let result = resolve_plugin_config_file(
        &info,
        Some(PluginConfigFile::Path("https://example.com/tombi.toml".into())),
        ConfigKeyMap::new(),
        Path::new("/project"),
        &environment,
        &mut states,
      )
      .unwrap();
      let ConfigKeyValue::Object(config) = &result[CONFIG_FILE_CONTENT_PROPERTY] else {
        panic!("missing config")
      };
      assert_eq!(config["source"].as_string().unwrap(), "https://cdn.example.com/team/tombi.toml");
      assert_eq!(config["absolutePath"], ConfigKeyValue::Null);
      let ConfigKeyValue::Object(documents) = &config["documents"] else {
        panic!("missing documents")
      };
      assert!(documents.contains_key("https://cdn.example.com/base/tombi.toml"));
      let ConfigKeyValue::Object(document) = &documents["https://cdn.example.com/base/tombi.toml"] else {
        panic!("missing document source")
      };
      assert_eq!(document["source"].as_string().unwrap(), "https://cdn.example.com/final/tombi.toml");
      let ConfigKeyValue::Object(files) = &config["files"] else {
        panic!("missing schemas")
      };
      assert_eq!(files.len(), 2);
    }
    for url in [
      "https://example.com/tombi.toml",
      "https://cdn.example.com/base/tombi.toml",
      "https://cdn.example.com/team/schema.json",
    ] {
      assert_eq!(environment.remote_file_download_count(url), 1, "{url}");
    }
    assert!(states.iter().all(|state| state.is_current(&environment)));
    environment.set_fs_time(5000);
    assert!(states.iter().all(|state| !state.is_current(&environment)));
  }

  #[test]
  fn remote_config_cannot_read_local_dependencies() {
    for text in ["extends = ['file:///secret/tombi.toml']", "[[schemas]]\npath = 'file:///secret/schema.json'"] {
      let environment = crate::environment::TestEnvironment::new();
      environment.add_remote_file_bytes("https://example.com/tombi.toml", text.as_bytes().to_vec());
      let mut info = tombi_info(Some(vec!["tombi.toml"]));
      info.config_reference_paths = vec![vec!["extends".into(), "*".into()]];
      info.config_schema_paths = vec![vec!["schemas".into(), "*".into(), "path".into()]];
      let error = resolve_plugin_config_file(
        &info,
        Some(PluginConfigFile::Path("https://example.com/tombi.toml".into())),
        ConfigKeyMap::new(),
        Path::new("/project"),
        &environment,
        &mut Vec::new(),
      )
      .unwrap_err();
      assert!(format!("{error:#}").contains("Remote configuration"), "{error:#}");
    }
  }

  #[test]
  fn plugin_declares_base_for_local_references_without_changing_remote_urls() {
    let mut info = tombi_info(Some(vec![".config/tombi.toml"]));
    info.config_relative_path_bases.insert(".config/tombi.toml".into(), "..".into());
    let file = url::Url::parse("file:///C:/project/.config/tombi.toml").unwrap();
    assert_eq!(
      config_reference_base(&info, &file).unwrap().join("base/tombi.toml").unwrap().as_str(),
      "file:///C:/project/base/tombi.toml"
    );
    let remote = url::Url::parse("https://example.com/.config/tombi.toml").unwrap();
    assert_eq!(config_reference_base(&info, &remote).unwrap(), remote);
    info
      .config_relative_path_bases
      .insert(".config/tombi.toml".into(), "https://elsewhere.example/".into());
    assert!(config_reference_base(&info, &file).is_err());
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
    schema_references(&schema, &base, &mut references, &mut Default::default()).unwrap();
    let mut references = references.iter().map(url::Url::as_str).collect::<Vec<_>>();
    references.sort_unstable();
    assert_eq!(references, ["file:///project/nested/entry.json", "file:///project/nested/setting.json"]);
  }

  #[test]
  fn loads_inline_schemas_without_a_plugin_config_file() {
    let environment = crate::environment::TestEnvironment::new();
    let root = if cfg!(windows) { "C:/project" } else { "/project" };
    let path = format!("{root}/schema.json");
    write(&environment, &path, r#"{"type":"object"}"#);
    for config_files in [None, Some(vec![]), Some(vec!["tombi.toml"])] {
      let mut info = tombi_info(config_files);
      info.config_schema_paths = vec![vec!["schema".into()]];
      for selected in ["schema.json", path.as_str()] {
        let mut states = Vec::new();
        let result = resolve_plugin_config_file(
          &info,
          None,
          properties(&[("schema", ConfigKeyValue::from_str(selected))]),
          Path::new(root),
          &environment,
          &mut states,
        )
        .unwrap();
        let ConfigKeyValue::Object(config) = &result[CONFIG_FILE_CONTENT_PROPERTY] else {
          panic!("missing resources")
        };
        let ConfigKeyValue::Object(files) = &config["files"] else {
          panic!("missing schemas")
        };
        assert_eq!(files.len(), 1);
        assert!(states.iter().all(|state| state.is_current(&environment)));
        write(&environment, &path, "{}");
        assert!(!states.iter().all(|state| state.is_current(&environment)));
        write(&environment, &path, r#"{"type":"object"}"#);
      }
    }
  }

  #[test]
  fn schema_ids_resolve_inside_the_loaded_document() {
    let environment = crate::environment::TestEnvironment::new();
    let root = if cfg!(windows) { "C:/project" } else { "/project" };
    write(
      &environment,
      &format!("{root}/schema.json"),
      r##"{
      "$id":"logical.json",
      "$defs":{"child":{"$id":"nested/child.json","type":"string","$ref":"#"}},
      "allOf":[{"$ref":"#/$defs/child"},{"$ref":"nested/child.json"}]
    }"##,
    );
    let mut info = tombi_info(Some(vec![]));
    info.config_schema_paths = vec![vec!["schema".into()]];
    let mut states = Vec::new();
    resolve_plugin_config_file(
      &info,
      None,
      properties(&[("schema", ConfigKeyValue::from_str("schema.json"))]),
      Path::new(root),
      &environment,
      &mut states,
    )
    .unwrap();
    assert_eq!(states.len(), 1);
  }

  #[test]
  fn plugin_can_resolve_local_inherited_schemas_from_the_root_config() {
    let environment = crate::environment::TestEnvironment::new();
    let root = if cfg!(windows) { "C:/project" } else { "/project" };
    write(&environment, &format!("{root}/tombi.toml"), "extends = ['base/tombi.toml']");
    write(&environment, &format!("{root}/base/tombi.toml"), "schema = 'schema.json'");
    write(&environment, &format!("{root}/schema.json"), r#"{"type":"object"}"#);
    write(&environment, &format!("{root}/base/schema.json"), r#"{"type":"string"}"#);
    let mut info = tombi_info(Some(vec!["tombi.toml"]));
    info.config_schema_paths = vec![vec!["schema".into()]];
    info.config_reference_paths = vec![vec!["extends".into(), "*".into()]];
    for (use_root, expected) in [(true, r#"{"type":"object"}"#), (false, r#"{"type":"string"}"#)] {
      info.config_local_schemas_use_root = use_root;
      let result = resolve_plugin_config_file(&info, None, properties(&[]), Path::new(root), &environment, &mut Vec::new()).unwrap();
      let ConfigKeyValue::Object(config) = &result[CONFIG_FILE_CONTENT_PROPERTY] else {
        panic!("missing config")
      };
      let ConfigKeyValue::Object(files) = &config["files"] else {
        panic!("missing schemas")
      };
      assert_eq!(files.values().next().unwrap().as_string().unwrap(), expected);
      assert_eq!(files.len(), 1);
    }
  }

  #[test]
  fn schema_limit_is_shared_across_inherited_configs() {
    let environment = crate::environment::TestEnvironment::new();
    let root = if cfg!(windows) { "C:/project" } else { "/project" };
    let mut info = tombi_info(Some(vec!["tombi.toml"]));
    info.config_schema_paths = vec![vec!["schemas".into(), "*".into()]];
    info.config_reference_paths = vec![vec!["extends".into(), "*".into()]];
    let schemas = (0..256).map(|i| format!("'s{i}.json'")).collect::<Vec<_>>().join(",");
    write(
      &environment,
      &format!("{root}/tombi.toml"),
      &format!("extends = ['base.toml']\nschemas = [{schemas}]"),
    );
    for i in 0..257 {
      write(&environment, &format!("{root}/s{i}.json"), "{}");
    }
    write(&environment, &format!("{root}/base.toml"), "schemas = ['s0.json']");
    let mut states = Vec::new();
    resolve_plugin_config_file(&info, None, properties(&[]), Path::new(root), &environment, &mut states).unwrap();
    assert_eq!(
      states
        .iter()
        .filter(|state| matches!(state, PluginConfigFileState::Confined {path, ..} if path.extension().is_some_and(|extension| extension == "json")))
        .count(),
      256
    );
    write(&environment, &format!("{root}/base.toml"), "schemas = ['s256.json']");
    let error = resolve_plugin_config_file(&info, None, properties(&[]), Path::new(root), &environment, &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("exceed 256 files"), "{error:#}");
  }
}
