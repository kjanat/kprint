#![allow(clippy::bool_assert_comparison)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::comparison_chain)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::vec_init_then_push)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_lifetimes)]
// Keep upstream control flow intact when newer Clippy suggests style changes.
#![allow(clippy::collapsible_if)]
// Keep the vendored public library surface available for upstream tests.
#![allow(dead_code, unused_imports)]
#![deny(clippy::disallowed_methods)]
#![deny(clippy::disallowed_types)]
#![deny(clippy::print_stderr)]
#![deny(clippy::print_stdout)]

pub mod configuration;
mod format_text;
mod generation;
mod html;
mod parser;

pub use format_text::format_text;
pub use format_text::FormatError;
pub use format_text::CodeBlockRequest;
pub use format_text::PreparedMarkdown;

#[cfg(any())]
pub use format_text::trace_file;
