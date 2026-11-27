use super::ast::Document;
use super::ast::Node;

/// Checks that the ast accounts for every byte of the html it was parsed from.
///
/// The children of a node run in order and butt up against one another, from
/// the first byte of what holds them to the last, so there is nowhere for text
/// to have been read and then dropped. That is what makes the ast something
/// the printer can write back out without losing anything.
pub fn validate_coverage(document: &Document<'_>, source: &str) -> Result<(), String> {
  tile("the document", document.children, source, 0, source.len(), source)
}

fn tile(within: &str, children: &[Node<'_>], content: &str, start: usize, end: usize, source: &str) -> Result<(), String> {
  if children.is_empty() {
    return if start == end {
      Ok(())
    } else {
      Err(format!("{} holds no children but covers {:?}", within, content))
    };
  }

  let mut position = start;
  for child in children {
    let text = child.source();
    let child_start = offset_in(text, source);
    if child_start != position {
      return Err(format!(
        "{}: the {} at {} doesn't start where the one before it ended, at {}",
        within,
        child.kind(),
        child_start,
        position,
      ));
    }
    if !source.is_char_boundary(child_start) || !source.is_char_boundary(child_start + text.len()) {
      return Err(format!("{}: the {} at {} splits a character", within, child.kind(), child_start));
    }
    position = child_start + text.len();

    if let Node::Element(element) = child {
      if element.content.is_empty() && element.children.is_empty() {
        continue;
      }
      let content_start = offset_in(element.content, source);
      let content_end = content_start + element.content.len();
      if content_start < child_start || content_end > position {
        return Err(format!("content of <{}> is outside its element", element.name));
      }
      let name = format!("`<{}>` at {}", element.name, child_start);
      tile(&name, element.children, element.content, content_start, content_end, source)?;
    }
  }

  if position != end {
    return Err(format!("{}: the children stop at {} but what holds them runs to {}", within, position, end));
  }
  Ok(())
}

fn offset_in(slice: &str, source: &str) -> usize {
  (slice.as_ptr() as usize).saturating_sub(source.as_ptr() as usize)
}
