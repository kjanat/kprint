use std::cell::Cell;

use kprint_formatting::IntoPrintItems;
use kprint_formatting::PrintItems;
use kprint_formatting::PrintOptions;
use kprint_formatting::conditions::if_true_or;

#[test]
fn should_stabilize_after_reevaluation_flipping() {
  let result = kprint_formatting::format(
    |arena| {
      let mut items = PrintItems::new(arena);
      let cell: &Cell<bool> = arena.bump().alloc(Cell::new(false));
      // cause an infinite loop
      let mut condition = if_true_or(
        "flipping",
        arena.alloc_resolver(move |_| {
          // flip forever
          cell.set(!cell.get());
          Some(cell.get())
        }),
        "1".into_print_items(arena),
        "2".into_print_items(arena),
      );
      let reevaluation = condition.create_reevaluation();
      items.push_condition(condition);
      items.push_reevaluation(reevaluation);
      items
    },
    PrintOptions {
      indent_width: 2,
      max_width: 40,
      use_tabs: false,
      new_line_text: "\n",
    },
  );
  assert_eq!(result, "1");
}
