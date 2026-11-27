use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

// Cargo runs build scripts before an environment exists.
#[allow(clippy::disallowed_methods)]
fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  // Keep alias precedence deterministic when multiple languages share an alias.
  let languages: BTreeMap<_, _> = linguist::definitions::LANGUAGES.iter().collect();
  let usable: Vec<_> = languages
    .iter()
    .filter_map(|(name, language)| {
      let extensions: Vec<_> = language
        .extensions
        .as_deref()?
        .iter()
        .filter_map(|ext| ext.strip_prefix('.'))
        .filter(|ext| {
          !ext.is_empty()
            && ext
              .bytes()
              .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
        })
        .collect();
      (!extensions.is_empty()).then_some((name, language, extensions))
    })
    .collect();

  let mut extensions = BTreeMap::new();
  let mut extension_groups = BTreeMap::new();
  for (name, language, candidates) in &usable {
    let group = language.group.as_deref().unwrap_or(name);
    for extension in candidates {
      extension_groups
        .entry(extension.to_ascii_lowercase())
        .or_insert_with(std::collections::BTreeSet::new)
        .insert(group);
    }
  }
  for (name, _, candidates) in &usable {
    extensions.insert(name.to_ascii_lowercase(), candidates);
  }
  for (_, language, candidates) in &usable {
    for alias in language.aliases.as_deref().unwrap_or_default() {
      extensions.entry(alias.to_ascii_lowercase()).or_insert(candidates);
    }
  }

  let mut generated = String::from("const LINGUIST_EXTENSIONS: &[(&str, &[&str])] = &[\n");
  for (name, candidates) in extensions {
    writeln!(generated, "  ({name:?}, &{candidates:?}),").unwrap();
  }
  generated.push_str("];\n");
  generated.push_str("const SHARED_EXTENSIONS: &[&str] = &[\n");
  for (extension, groups) in extension_groups {
    if groups.len() > 1 {
      writeln!(generated, "  {extension:?},").unwrap();
    }
  }
  generated.push_str("];\n");
  let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("language_extensions.rs");
  std::fs::write(output, generated).unwrap();
}
