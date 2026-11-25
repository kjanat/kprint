use std::collections::BTreeMap;
use std::fmt::Write;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;

// Cargo runs build scripts before an environment exists.
#[allow(clippy::disallowed_methods)]
fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  // Keep alias precedence deterministic when multiple languages share an alias.
  let languages: BTreeMap<_, _> = linguist::definitions::LANGUAGES.iter().collect();
  let mut definitions = Vec::new();
  let mut extension_counts = BTreeMap::new();
  for (name, language) in &languages {
    let extensions = language
      .extensions
      .as_deref()
      .unwrap_or_default()
      .iter()
      .filter_map(|ext| ext.strip_prefix('.').filter(|ext| safe_file_name(ext)))
      .collect::<Vec<_>>();
    let filenames = language
      .filenames
      .as_deref()
      .unwrap_or_default()
      .iter()
      .map(String::as_str)
      .filter(|name| safe_file_name(name))
      .collect::<Vec<_>>();
    if extensions.is_empty() && filenames.is_empty() {
      continue;
    }
    for extension in &extensions {
      *extension_counts.entry(extension.to_ascii_lowercase()).or_insert(0u32) += 1;
    }
    definitions.push((name.to_ascii_lowercase(), language, extensions, filenames));
  }

  let mut aliases = definitions
    .iter()
    .enumerate()
    .map(|(index, (name, ..))| (name.clone(), index))
    .collect::<BTreeMap<_, _>>();
  for (index, (_, language, ..)) in definitions.iter().enumerate() {
    for alias in language.aliases.as_deref().unwrap_or_default() {
      aliases.entry(alias.to_ascii_lowercase()).or_insert(index);
    }
  }

  let mut generated = String::from("const LINGUIST_LANGUAGES: &[LanguageDefinition] = &[\n");
  for (name, _, extensions, filenames) in definitions {
    writeln!(
      generated,
      "  LanguageDefinition {{ name: {name:?}, extensions: &{extensions:?}, filenames: &{filenames:?} }},"
    )
    .unwrap();
  }
  generated.push_str("];\nconst LINGUIST_ALIASES: &[(&str, usize)] = &[\n");
  for (name, index) in aliases {
    writeln!(generated, "  ({name:?}, {index}),").unwrap();
  }
  generated.push_str("];\nconst SHARED_EXTENSIONS: &[&str] = &[\n");
  for (extension, count) in extension_counts {
    if count > 1 {
      writeln!(generated, "  {extension:?},").unwrap();
    }
  }
  generated.push_str("];\n");
  let mut hasher = std::hash::DefaultHasher::new();
  generated.hash(&mut hasher);
  writeln!(generated, "const LINGUIST_CATALOG_HASH: u64 = {};", hasher.finish()).unwrap();
  let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("language_extensions.rs");
  std::fs::write(output, generated).unwrap();
}

fn safe_file_name(name: &str) -> bool {
  !name.is_empty()
    && !matches!(name, "." | "..")
    && name
      .bytes()
      .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}
