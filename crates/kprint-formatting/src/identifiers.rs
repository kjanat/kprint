use std::cell::Cell;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

/// Never reused, including after an arena is dropped or its allocation is pooled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ArenaId(usize);

impl ArenaId {
  pub fn new() -> Self {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    Self(
      NEXT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("Arena identifier space exhausted"),
    )
  }

  pub fn assert_matches(self, other: Self) {
    assert_eq!(self, other, "Identifiers must belong to the same arena.");
  }
}

#[derive(Default)]
pub(super) struct Counter(Cell<u32>);

impl Counter {
  pub fn next(&self) -> u32 {
    let value = self.0.get();
    self.0.set(value.checked_add(1).expect("Arena identifier space exhausted"));
    value
  }

  pub fn count(&self) -> u32 {
    self.0.get()
  }
}

#[derive(Default)]
pub(super) struct Counts {
  pub line_number_anchor_id: Counter,
  pub line_number_id: Counter,
  pub column_number_id: Counter,
  pub is_start_of_line_id: Counter,
  pub indent_level_id: Counter,
  pub line_start_column_number_id: Counter,
  pub line_start_indent_level_id: Counter,
  pub condition_id: Counter,
  pub condition_reevaluation_id: Counter,
  #[cfg(feature = "tracing")]
  pub print_node_id: Counter,
  #[cfg(feature = "tracing")]
  pub graph_node_id: Counter,
}
