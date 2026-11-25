use std::collections::BTreeMap;
use std::fmt::Write;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

const LANGUAGES: &str = "https://raw.githubusercontent.com/github-linguist/linguist/refs/heads/main/lib/linguist/languages.yml";
const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct Language {
  #[serde(default)]
  aliases: Vec<String>,
  #[serde(default)]
  extensions: Vec<String>,
}

fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  let response = ureq::AgentBuilder::new()
    .timeout(Duration::from_secs(30))
    .build()
    .get(LANGUAGES)
    .call()
    .unwrap_or_else(|error| panic!("Failed to fetch GitHub Linguist language definitions: {error}"));
  let mut yaml = Vec::new();
  response.into_reader().take(MAX_SOURCE_BYTES + 1).read_to_end(&mut yaml).unwrap();
  assert!(yaml.len() as u64 <= MAX_SOURCE_BYTES, "GitHub Linguist language definitions exceed 1 MiB");
  let languages: BTreeMap<String, Language> = serde_norway::from_slice(&yaml).expect("Invalid GitHub Linguist language definitions");
  let usable = languages.iter().filter_map(|(name, language)| {
    let extension = language.extensions.iter().find_map(|ext| {
      let ext = ext.strip_prefix('.')?;
      (!ext.is_empty()
        && ext != "ipynb"
        && ext
          .bytes()
          .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')))
      .then_some(ext)
    })?;
    Some((name, language, extension))
  });

  let mut extensions = BTreeMap::new();
  for (name, _, extension) in usable.clone() {
    extensions.insert(name.to_ascii_lowercase(), extension);
  }
  for (_, language, extension) in usable {
    for alias in &language.aliases {
      extensions.entry(alias.to_ascii_lowercase()).or_insert(extension);
    }
  }

  let mut generated = String::from("const LINGUIST_EXTENSIONS: &[(&str, &str)] = &[\n");
  for (name, extension) in extensions {
    writeln!(generated, "  ({name:?}, {extension:?}),").unwrap();
  }
  generated.push_str("];\n");
  let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("language_extensions.rs");
  std::fs::write(output, generated).unwrap();
}
