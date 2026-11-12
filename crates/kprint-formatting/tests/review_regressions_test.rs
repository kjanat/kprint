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

#[test]
fn nested_arenas_keep_independent_dense_identifiers() {
  let outer = Arena::new();
  let first = LineNumber::new(&outer, "first");
  let inner = Arena::new();
  let second = LineNumber::new(&outer, "second");
  let inner_line = LineNumber::new(&inner, "inner");
  assert_eq!(inner_line.unique_id(), 0);
  let mut inner_items = PrintItems::new(&inner);
  inner_items.push_info(inner_line);
  inner_items.extend(actions::action(&inner, "check inner", move |context| {
    assert_eq!(context.resolved_line_number(inner_line), Some(0));
  }));
  assert_eq!(print(inner_items, options()), "");
  drop(inner);
  let third = LineNumber::new(&outer, "third");
  assert_eq!([first.unique_id(), second.unique_id(), third.unique_id()], [0, 1, 2]);
  let mut items = PrintItems::new(&outer);
  for (line, text) in [(first, "a"), (second, "b"), (third, "c")] {
    items.push_info(line);
    items.push_str(text);
    items.push_signal(Signal::NewLine);
  }
  items.extend(actions::action(&outer, "check outer", move |context| {
    assert_eq!(context.resolved_line_number(first), Some(0));
    assert_eq!(context.resolved_line_number(second), Some(1));
    assert_eq!(context.resolved_line_number(third), Some(2));
  }));
  assert_eq!(print(items, options()), "a\nb\nc\n");
}

#[test]
fn non_lifo_destruction_does_not_reset_surviving_identifiers() {
  let first = Arena::new();
  let second = Arena::new();
  let before = LineNumber::new(&second, "before");
  drop(first);
  let after = LineNumber::new(&second, "after");
  assert_eq!([before.unique_id(), after.unique_id()], [0, 1]);
  // An arena can also move to another thread before its items borrow it.
  std::thread::spawn(move || {
    assert_eq!(LineNumber::new(&second, "moved").unique_id(), 2);
  })
  .join()
  .unwrap();
}

#[test]
fn identifiers_from_dropped_pooled_arenas_are_rejected() {
  let pool = ArenaPool::new();
  let (infos, line, anchor, reference, reevaluation) = {
    let arena = pool.create_arena();
    let line = LineNumber::new(&arena, "stale");
    let infos = [
      Info::from(line),
      ColumnNumber::new(&arena, "stale").into(),
      IsStartOfLine::new(&arena, "stale").into(),
      IndentLevel::new(&arena, "stale").into(),
      LineStartColumnNumber::new(&arena, "stale").into(),
      LineStartIndentLevel::new(&arena, "stale").into(),
    ];
    let mut condition = Condition::new_true(&arena);
    (
      infos,
      line,
      LineNumberAnchor::new(&arena, line),
      condition.create_reference(),
      condition.create_reevaluation(),
    )
  };
  let arena = pool.create_arena();
  let current = LineNumber::new(&arena, "current");
  assert_eq!(line.unique_id(), current.unique_id());
  assert_ne!(line, current);
  let mut items = "unchanged".into_print_items(&arena);
  for info in infos {
    assert!(catch_unwind(AssertUnwindSafe(|| items.push_info(info))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| items.push_item(PrintItem::Info(info)))).is_err());
  }
  assert!(catch_unwind(AssertUnwindSafe(|| items.push_anchor(anchor.clone()))).is_err());
  assert!(catch_unwind(AssertUnwindSafe(|| LineNumberAnchor::new(&arena, line))).is_err());
  assert!(catch_unwind(AssertUnwindSafe(|| items.push_reevaluation(reevaluation))).is_err());
  assert!(catch_unwind(AssertUnwindSafe(|| reference.create_resolver(&arena))).is_err());
  let region = ir_helpers::InfoRegion::new(&arena);
  assert!(catch_unwind(AssertUnwindSafe(|| region.add(line))).is_err());
  assert_eq!(print(items, options()), "unchanged");

  for info in infos {
    rejects_stale_resolver(move |context| match info {
      Info::LineNumber(info) => {
        context.resolved_line_number(info);
      }
      Info::ColumnNumber(info) => {
        context.resolved_column_number(info);
      }
      Info::IsStartOfLine(info) => {
        context.resolved_is_start_of_line(info);
      }
      Info::IndentLevel(info) => {
        context.resolved_indent_level(info);
      }
      Info::LineStartColumnNumber(info) => {
        context.resolved_line_start_column_number(info);
      }
      Info::LineStartIndentLevel(info) => {
        context.resolved_line_start_indent_level(info);
      }
    });
    rejects_stale_resolver(move |context| context.clear_info(info));
  }
  rejects_stale_resolver(move |context| {
    context.peek_line_number(line);
  });
  rejects_stale_resolver(move |context| context.set_line_number(line, 10));
  rejects_stale_resolver(move |context| context.shift_line_number(line, 1));
  rejects_stale_resolver(move |context| {
    context.resolved_condition(&reference);
  });
}

fn rejects_stale_resolver(action: impl Fn(&mut ConditionResolverContext) + Copy) {
  let arena = Arena::new();
  let items = actions::action(&arena, "stale resolver", action);
  assert!(catch_unwind(AssertUnwindSafe(|| print(items, options()))).is_err());
}

#[test]
fn conditions_and_regions_cannot_cross_arenas_even_without_paths() {
  let outer = Arena::new();
  let inner = Arena::new();
  let mut items = "unchanged".into_print_items(&outer);
  let condition = Condition::new_true(&inner);
  assert!(catch_unwind(AssertUnwindSafe(|| items.push_condition(condition.clone()))).is_err());
  assert!(
    catch_unwind(AssertUnwindSafe(|| Condition::new(
      &outer,
      "wrong path",
      ConditionProperties {
        condition: condition_resolvers::true_resolver(),
        true_path: Some(PrintItems::new(&inner)),
        false_path: None,
      },
    )))
    .is_err()
  );
  let region = ir_helpers::InfoRegion::new(&outer);
  assert!(catch_unwind(AssertUnwindSafe(|| region.gen_start(&inner))).is_err());
  assert_eq!(print(items, options()), "unchanged");
}
