#[macro_use]
extern crate kprint_platform;
mod git_index;
mod git_repo;
mod gitignore;
mod glob;
mod path_case;
mod repo_index;

mod config_patterns;
pub use config_patterns::*;
pub use gitignore::*;
pub use glob::*;
pub use kprint_host_api::selection::*;
pub mod environment {
  pub use kprint_platform::environment::*;
  #[cfg(test)]
  pub use kprint_test_support::environment::*;
}
pub const POSSIBLE_CONFIG_FILE_NAMES: &[&str] = &["dprint.json", "dprint.jsonc", ".dprint.json", ".dprint.jsonc", "dprint.toml", ".dprint.toml"];
mod utils {
  pub(crate) use crate::gitignore;
  pub use crate::glob::*;
}
#[cfg(test)]
pub use kprint_test_support::test_helpers;

/// Project configuration candidates, with root files preferred over `.config/`.
pub fn config_file_names<'a>(dir: &std::path::Path, environment: &impl environment::DirectoriesEnvironment) -> impl Iterator<Item = &'a str> {
  let search_dot_config = environment.get_home_dir().is_none_or(|home| home.as_ref() != dir);
  POSSIBLE_CONFIG_FILE_NAMES.iter().copied().chain(
    [".config/dprint.json", ".config/dprint.jsonc", ".config/dprint.toml"]
      .into_iter()
      .filter(move |_| search_dot_config),
  )
}

pub fn is_project_config_dir(dir: &std::path::Path, environment: &impl environment::DirectoriesEnvironment) -> bool {
  dir.file_name().is_some_and(|name| name == ".config")
    && dir
      .parent()
      .is_some_and(|parent| environment.get_home_dir().is_none_or(|home| home.as_ref() != parent))
}
