use kprint_host::resolve_language_extension;

/// Gets a synthetic file name for a document with no file on disk.
pub fn get_language_file_name(base_name: &str, language_id: &str) -> Option<String> {
  let language = language_id.to_ascii_lowercase();
  let ext = match language.as_str() {
    // Editor-specific IDs and mappings shared with the Jupyter formatter.
    "handlebars" => "hbs",
    "javascriptreact" => "jsx",
    "proto3" => "proto",
    "shellscript" => "sh",
    "typescriptreact" => "tsx",
    _ => resolve_language_extension(&language)
      .or_else(|| (!language.is_empty() && language.bytes().all(|byte| byte.is_ascii_alphanumeric())).then_some(language.as_str()))?,
  };
  // Routing a cell to a notebook formatter would recurse into the notebook.
  if ext == "ipynb" {
    return None;
  }
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
      ("hcl", "cell.hcl"),
      ("terraform", "cell.hcl"),
      ("shellscript", "cell.sh"),
    ] {
      assert_eq!(get_language_file_name("cell", id).as_deref(), Some(name));
    }
    assert_eq!(get_language_file_name("cell", "ipynb"), None);
    assert_eq!(get_language_file_name("cell", "jupyter notebook"), None);
  }
}
