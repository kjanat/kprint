use std::ops::Range;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::bail;
use similar::Algorithm;
use similar::DiffTag;

/// Translates a byte selection through a plugin's edits before another plugin
/// formats it. Edits outside the selection shift its coordinates, but aren't
/// added to the selection. Ambiguous boundaries fail instead of widening it.
pub fn translate_format_range(old: &[u8], new: &[u8], range: Range<usize>) -> Result<Range<usize>> {
  if range.start > range.end || range.end > old.len() {
    bail!("The formatting range is outside the plugin's input.");
  }
  if old == new {
    return Ok(range);
  }
  let edits = similar::capture_diff_slices_deadline(Algorithm::Myers, old, new, Some(Instant::now() + Duration::from_millis(100)));
  let map = |position: usize| -> Result<usize> {
    for edit in &edits {
      let before = edit.old_range();
      let after = edit.new_range();
      if edit.tag() == DiffTag::Equal && before.contains(&position) {
        return Ok(after.start + position - before.start);
      }
      if position == before.start {
        return Ok(after.start);
      }
      if position == before.end {
        return Ok(after.end);
      }
      if before.contains(&position) {
        bail!("Cannot preserve the formatting selection: a plugin changed text across its boundary.");
      }
    }
    Ok(new.len())
  };
  Ok(map(range.start)?..map(range.end)?)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn preserves_the_selection_through_shrinking_and_growing_edits() {
    assert_eq!(translate_format_range(b"abcdefghijklmnopqrst\nTAIL", b"_x_\nTAIL_x", 0..20).unwrap(), 0..3);
    assert_eq!(translate_format_range(b"HEAD\nx\nTAIL", b"HEAD\nlonger\nTAIL", 5..6).unwrap(), 5..11);
    assert_eq!(translate_format_range(b"HEAD\nx\nTAIL", b"NEW HEAD\nx\nTAIL!", 5..6).unwrap(), 9..10);
    assert_eq!(translate_format_range("H\né\nT".as_bytes(), "H\n世界\nT!".as_bytes(), 2..4).unwrap(), 2..8);
  }

  #[test]
  fn refuses_to_guess_when_a_plugin_changes_across_a_boundary() {
    assert!(translate_format_range(b"abcdef", b"XYZ", 2..4).is_err());
  }
}
