mod command;
pub mod input;
pub mod legacy;
pub use command::split_command;
pub use legacy::is_exec_plugin_reference;
/// What a configuration's plugins can list to name the built-in exec.
pub const BUILT_IN_REFERENCE_NAME: &str = "exec";

/// Compatibility reference used internally for the built-in selector.
/// Every recognized exec reference resolves to the host formatter.
pub const BUILT_IN_REFERENCE: &str = "npm:@dprint/exec@0.7.3/plugin.json";
