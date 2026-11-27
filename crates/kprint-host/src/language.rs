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
    let Some(owners) = shared_extension_owners(extension) else {
      return true;
    };
    if self.label == "txt" || self.label.eq_ignore_ascii_case(config_key) {
      return true;
    }
    let formatter_language = get_definition(&config_key.to_ascii_lowercase());
    let supports_extension = |extension: &str| supported_extensions.iter().any(|supported| supported.eq_ignore_ascii_case(extension));
    let has_unique_extension = |extensions: &[&str]| {
      extensions
        .iter()
        .any(|extension| shared_extension_owners(extension).is_none() && supports_extension(extension))
    };
    if self
      .definition
      .is_some_and(|definition| formatter_language.is_some_and(|language| language.name == definition.name) || has_unique_extension(&self.extensions))
    {
      return true;
    }

    // A generic formatter can serve a uniquely preferred extension if
    // its claims don't identify any of the alternative owners.
    let owners = || owners.iter().map(|index| &LINGUIST_LANGUAGES[*index]);
    let mut preferred = owners().filter(|owner| owner.extensions.first().is_some_and(|ext| ext.eq_ignore_ascii_case(extension)));
    let Some(preferred_owner) = preferred.next() else {
      return false;
    };
    preferred.next().is_none()
      && match self.definition {
        Some(definition) => definition.name == preferred_owner.name,
        None => self.is_unregistered_extension() && self.label.eq_ignore_ascii_case(extension),
      }
      && !owners().filter(|owner| owner.name != preferred_owner.name).any(|owner| {
        formatter_language.is_some_and(|language| language.name == owner.name)
          || owner
            .extensions
            .iter()
            .any(|extension| !preferred_owner.extensions.iter().any(|preferred| preferred.eq_ignore_ascii_case(extension)) && supports_extension(extension))
      })
  }
}

fn shared_extension_owners(extension: &str) -> Option<&'static [usize]> {
  let lowered = extension.to_ascii_lowercase();
  let index = SHARED_EXTENSION_OWNERS.binary_search_by_key(&lowered.as_str(), |(ext, _)| *ext).ok()?;
  Some(SHARED_EXTENSION_OWNERS[index].1)
}

pub(crate) fn hash_language_routing(hasher: &mut impl std::hash::Hasher) {
  use std::hash::Hash;
  // The format result can change when an installed plugin starts matching an
  // alias following a Linguist update. Keep that state in the incremental key.
  const ROUTING_VERSION: u8 = 3;
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
  assert!(!objective_c.allows_formatter("m", "exec", &["m".into()]));
  assert!(objective_c.allows_formatter("m", "objective-c", &["m".into()]));
  let docker = LanguageCandidates::new("Dockerfile").unwrap();
  assert_eq!(docker.filenames().next(), Some("Dockerfile"));
  assert_eq!(LanguageCandidates::new("Terraform").unwrap().extensions().next(), Some("tf"));
}

#[test]
fn a_formatter_can_claim_a_shared_extension_uniquely_preferred_by_the_requested_language() {
  let csharp = LanguageCandidates::new("csharp").unwrap();
  assert!(csharp.allows_formatter("cs", "exec", &["cs".into()]));
  assert!(LanguageCandidates::new("cs").unwrap().allows_formatter("cs", "exec", &["cs".into()]));
  assert!(!csharp.allows_formatter("cs", "smalltalk", &["cs".into()]));
  assert!(!csharp.allows_formatter("cs", "squeak", &["cs".into()]));
  assert!(!csharp.allows_formatter("cs", "formatter", &["st".into(), "cs".into()]));
  assert!(!LanguageCandidates::new("smalltalk").unwrap().allows_formatter("cs", "exec", &["cs".into()]));
}
