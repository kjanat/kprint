//! Markdown parsing and layout adapted from dprint-plugin-markdown 0.29.0
//! (742812395bf000749afba185e68f44414101cecb, MIT, see LICENSE).
//!
//! The formatter reports fenced content to its caller. Language resolution,
//! delegation and diagnostics belong to the host.

pub mod configuration;
mod format_range;
mod format_text;
mod generation;
mod html;
mod parser;

pub use format_range::format_text_range;
pub use format_text::FormatError;
pub use format_text::format_text;

pub const LICENSE: &str = include_str!("../LICENSE");

/// A block of embedded content encountered during Markdown layout.
pub struct EmbeddedCode<'a> {
  /// Byte offset of the opening delimiter in the original input.
  pub offset: usize,
  pub language: &'a str,
  pub text: &'a str,
  pub line_width: u32,
}

#[cfg(feature = "tracing")]
pub use format_text::trace_file;
