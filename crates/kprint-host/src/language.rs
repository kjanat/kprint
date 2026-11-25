use std::path::Path;
use std::path::PathBuf;

struct LanguageDefinition {
  name: &'static str,
  extensions: &'static [&'static str],
  filenames: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/language_extensions.rs"));

fn get_definition(language_id: &str) -> Option<&'static LanguageDefinition> {
  let index = LINGUIST_ALIASES.binary_search_by_key(&language_id, |(id, _)| *id).ok()?;
  Some(&LINGUIST_LANGUAGES[LINGUIST_ALIASES[index].1])
}

/// Looks up Linguist's preferred extension for a language name or alias.
pub fn resolve_language_extension(language_id: &str) -> Option<&'static str> {
  get_definition(&language_id.to_ascii_lowercase())?.extensions.first().copied()
}

pub(crate) struct LanguageCandidates {
  label: String,
  definition: Option<&'static LanguageDefinition>,
  extensions: Vec<&'static str>,
  use_label_as_extension: bool,
}

impl LanguageCandidates {
  pub(crate) fn new(language: &str) -> Option<Self> {
    let label = language.trim().to_ascii_lowercase();
    if label.is_empty() || label.len() > 128 || label.chars().any(char::is_control) {
      return None;
    }
    let definition = get_definition(&label);
    // Linguist groups shell dialects for syntax highlighting and groups HCL
    // dialects by grammar. A formatter's filename can distinguish them.
    let extensions: &[&str] = match label.as_str() {
      "bash" => &["bash"],
      "zsh" => &["zsh"],
      "ksh" => &["ksh"],
      "sh" | "shell" | "shell-script" | "shellscript" => &["sh"],
      "terraform" => &["tf", "tfvars"],
      "opentofu" => &["tofu"],
      "hcl" | "hashicorp configuration language" => &["hcl"],
      "handlebars" => &["hbs"],
      "javascriptreact" => &["jsx"],
      "typescriptreact" => &["tsx"],
      "proto3" => &["proto"],
      "cmd" => &["cmd"],
      // Plain text can be useful with a plugin configured to format it.
      "txt" => &["txt"],
      _ => definition.map(|language| language.extensions).unwrap_or_default(),
    };
    // A short unregistered token can be an extension (e.g. a plugin-specific
    // extension). Only allow characters that cannot become path components.
    let use_label_as_extension = definition.is_none()
      && extensions.is_empty()
      && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+'))
      && label.bytes().any(|byte| byte.is_ascii_alphanumeric());
    Some(Self {
      label,
      definition,
      extensions: extensions.to_vec(),
      use_label_as_extension,
    })
  }

  pub(crate) fn extensions(&self) -> impl Iterator<Item = &str> {
    self
      .extensions
      .iter()
      .copied()
      .chain(self.use_label_as_extension.then_some(self.label.as_str()))
  }

  pub(crate) fn is_unregistered_extension(&self) -> bool {
    self.use_label_as_extension
  }

  pub(crate) fn selectors(&self) -> impl Iterator<Item = &str> {
    std::iter::once(self.label.as_str()).chain(self.extensions().filter(|ext| *ext != self.label))
  }

  pub(crate) fn filenames(&self) -> impl Iterator<Item = &str> {
    let files = self
      .definition
      .filter(|language| language.name != "shell")
      .map(|language| language.filenames)
      .unwrap_or_default();
    files
      .iter()
      .copied()
      .filter(move |file| file.eq_ignore_ascii_case(&self.label))
      .chain(files.iter().copied().filter(move |file| !file.eq_ignore_ascii_case(&self.label)))
  }

  pub(crate) fn virtual_path(&self, base: &Path, extension: Option<&str>) -> PathBuf {
    match extension {
      Some(extension) => base.with_extension(extension),
      None => self
        .extensions()
        .next()
        .map(|extension| base.with_extension(extension))
        .unwrap_or_else(|| base.to_path_buf()),
    }
  }

  pub(crate) fn allows_formatter(&self, extension: &str, config_key: &str, supported_extensions: &[String]) -> bool {
    !is_shared_extension(extension)
      || self.label == "txt"
      || self.label.eq_ignore_ascii_case(config_key)
      || self.definition.is_some_and(|definition| {
        definition.name.eq_ignore_ascii_case(config_key)
          || self
            .extensions()
            .filter(|extension| !is_shared_extension(extension))
            .any(|extension| supported_extensions.iter().any(|supported| supported.eq_ignore_ascii_case(extension)))
      })
  }
}

fn is_shared_extension(extension: &str) -> bool {
  SHARED_EXTENSIONS.binary_search(&extension.to_ascii_lowercase().as_str()).is_ok()
}

pub(crate) fn hash_language_routing(hasher: &mut impl std::hash::Hasher) {
  use std::hash::Hash;
  // The format result can change when an installed plugin starts matching an
  // alias following a Linguist update. Keep that state in the incremental key.
  const ROUTING_VERSION: u8 = 1;
  ROUTING_VERSION.hash(hasher);
  LINGUIST_CATALOG_HASH.hash(hasher);
}

#[test]
fn resolves_linguist_names_and_aliases_to_extensions() {
  assert_eq!(resolve_language_extension("HCL"), Some("hcl"));
  assert_eq!(resolve_language_extension("terraform"), Some("hcl"));
  assert_eq!(resolve_language_extension("objc"), Some("m"));
  assert_eq!(resolve_language_extension("Jupyter Notebook"), Some("ipynb"));
  assert_eq!(resolve_language_extension("unknown-language"), None);
}

#[test]
fn language_candidates_keep_shell_dialects_and_require_evidence_for_shared_extensions() {
  let bash = LanguageCandidates::new(" Bash ").unwrap();
  assert_eq!(bash.extensions().collect::<Vec<_>>(), ["bash"]);
  assert_eq!(LanguageCandidates::new("zsh").unwrap().extensions().collect::<Vec<_>>(), ["zsh"]);
  let objective_c = LanguageCandidates::new("objc").unwrap();
  assert!(!objective_c.allows_formatter("m", "matlab", &["m".into()]));
  assert!(objective_c.allows_formatter("m", "objective-c", &["m".into()]));
  let docker = LanguageCandidates::new("Dockerfile").unwrap();
  assert_eq!(docker.filenames().next(), Some("Dockerfile"));
  assert_eq!(LanguageCandidates::new("Terraform").unwrap().extensions().next(), Some("tf"));
}
