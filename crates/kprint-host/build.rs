use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

// Cargo runs build scripts before an environment exists.
#[allow(clippy::disallowed_methods)]
fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  // Keep alias precedence deterministic when multiple languages share an alias.
  let languages: BTreeMap<_, _> = linguist::definitions::LANGUAGES.iter().collect();
  let usable = languages.iter().filter_map(|(name, language)| {
    let extension = language.extensions.as_deref()?.iter().find_map(|ext| {
      let ext = ext.strip_prefix('.')?;
      (!ext.is_empty()
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
    for alias in language.aliases.as_deref().unwrap_or_default() {
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
