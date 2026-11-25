include!(concat!(env!("OUT_DIR"), "/language_extensions.rs"));

/// Gets a synthetic file name for a document with no file on disk.
pub fn get_language_file_name(base_name: &str, language_id: &str) -> Option<String> {
  let language = language_id.to_ascii_lowercase();
  let ext = match language.as_str() {
    // Editor-specific IDs and mappings shared with the Jupyter formatter.
    "handlebars" => "hbs",
    "javascriptreact" => "jsx",
    "proto3" => "proto",
    "shellscript" => "sh",
    "terraform" => "tf",
    "typescriptreact" => "tsx",
    "ipynb" => return None,
    _ => match LINGUIST_EXTENSIONS.binary_search_by_key(&language.as_str(), |(id, _)| *id) {
      Ok(index) => LINGUIST_EXTENSIONS[index].1,
      Err(_) if !language.is_empty() && language.bytes().all(|byte| byte.is_ascii_alphanumeric()) => &language,
      Err(_) => return None,
    },
  };
  Some(format!("{base_name}.{ext}"))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn uses_linguist_names_and_aliases_alongside_editor_ids() {
    for (id, name) in [
      ("objective-c", "cell.m"),
      ("objc", "cell.m"),
      ("CSharp", "cell.cs"),
      ("typescriptreact", "cell.tsx"),
      ("terraform", "cell.tf"),
      ("shellscript", "cell.sh"),
    ] {
      assert_eq!(get_language_file_name("cell", id).as_deref(), Some(name));
    }
    assert_eq!(get_language_file_name("cell", "ipynb"), None);
    assert_eq!(get_language_file_name("cell", "jupyter notebook"), None);
  }
}
