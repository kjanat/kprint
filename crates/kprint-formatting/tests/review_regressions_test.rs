use kprint_formatting::*;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

fn options() -> PrintOptions {
  PrintOptions {
    indent_width: 2,
    max_width: 40,
    use_tabs: false,
    new_line_text: "\n",
  }
}

#[test]
fn cross_arena_composition_is_rejected_before_mutating_items() {
  let outer = Arena::new();
  let mut items = "before".into_print_items(&outer);
  let inner = Arena::new();
  assert!(catch_unwind(AssertUnwindSafe(|| items.extend("wrong".into_print_items(&inner)))).is_err());
  assert!(catch_unwind(AssertUnwindSafe(|| "wrong".into_print_items(&inner).into_print_items(&outer))).is_err());
  let path = "wrong".into_print_items(&inner).into_rc_path().unwrap();
  assert!(catch_unwind(AssertUnwindSafe(|| items.push_path(path))).is_err());
  assert!(catch_unwind(AssertUnwindSafe(|| items.push_item(PrintItem::RcPath(path)))).is_err());
  assert!(
    catch_unwind(AssertUnwindSafe(|| items.push_condition(conditions::if_true(
      "wrong arena",
      condition_resolvers::true_resolver(),
      "wrong".into_print_items(&inner),
    ))))
    .is_err()
  );
  items.extend("after".into_print_items(&outer));
  assert_eq!(print(items, options()), "beforeafter");
}

#[test]
fn resetting_indent_inside_a_region_does_not_underflow_its_width() {
  assert_eq!(
    format(
      |arena| {
        ir_helpers::with_indent(ir_helpers::surround_with_newlines_indented_if_multi_line(
          ir_helpers::with_reset_indent("x".into_print_items(arena)),
          2,
        ))
      },
      options()
    ),
    "x"
  );
}

#[test]
fn conditional_space_does_not_leave_alignment_on_a_blank_line() {
  assert_eq!(
    format(
      |arena| {
        let mut items = PrintItems::new(arena);
        items.push_signal(Signal::StartAlign(3));
        items.push_signal(Signal::SpaceIfNotTrailing);
        items.push_signal(Signal::NewLine);
        items.push_str_runtime_width_computed("x");
        items.push_signal(Signal::SpaceIfNotTrailing);
        items.push_signal(Signal::NewLine);
        items.push_signal(Signal::FinishAlign);
        items
      },
      options()
    ),
    "\n   x\n"
  );
}
