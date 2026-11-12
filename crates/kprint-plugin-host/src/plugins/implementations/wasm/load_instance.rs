use std::path::Path;

use anyhow::Result;
use anyhow::bail;
use wasmtime::Config;
use wasmtime::Engine;
use wasmtime::Func;
use wasmtime::Memory;
use wasmtime::Module;

use super::PluginSchemaVersion;
use super::instance::Linker;
use super::instance::Store;
use super::instance::WasmHostState;
use super::instance::get_current_plugin_schema_version;

pub struct WasmInstance {
  inner: wasmtime::Instance,
  // note: keep the engine alive for the duration of the instance
  // otherwise it could be cleaned up before the instance is dropped
  _engine: wasmtime::Engine,
  version: PluginSchemaVersion,
}

impl WasmInstance {
  pub fn version(&self) -> PluginSchemaVersion {
    self.version
  }

  pub fn get_memory(&self, store: &mut Store, name: &str) -> Option<Memory> {
    self.inner.get_memory(store, name)
  }

  pub fn get_function(&self, store: &mut Store, name: &str) -> Option<Func> {
    self.inner.get_func(store, name)
  }
}

/// Instantiates a compiled wasm module with the given linker.
pub fn load_instance(store: &mut Store, module: &WasmModule, linker: &Linker) -> Result<WasmInstance> {
  let mut linker = linker.clone();
  super::instance::wasi::native::add_unsupported_wasi_imports(&mut linker, store, &module.inner)?;
  let instance = match linker.instantiate(&mut *store, &module.inner) {
    Ok(instance) => instance,
    Err(err) => bail!("Error instantiating module: {:#}", err),
  };
  if !module.initializes_on_start
    && let Some(initialize) = instance.get_func(&mut *store, "_initialize")
  {
    initialize
      .typed::<(), ()>(&*store)?
      .call(&mut *store, ())
      .map_err(|err| anyhow::anyhow!("Error initializing module: {:#}", err))?;
  } else if instance.get_func(&mut *store, "_start").is_some() && instance.get_func(&mut *store, "_initialize").is_none() {
    bail!("WASI plugins must be built as reactors exporting _initialize, not commands exporting _start.");
  }
  Ok(WasmInstance {
    inner: instance,
    _engine: module.engine.clone(),
    version: module.version,
  })
}

#[derive(Clone)]
pub struct WasmModule {
  inner: wasmtime::Module,
  engine: wasmtime::Engine,
  version: PluginSchemaVersion,
  initializes_on_start: bool,
}

pub fn plugin_initializes_on_start(info: &kprint_plugin_types::PluginInfo) -> bool {
  info.name == "dprint-plugin-gofumpt"
    && deno_semver::Version::parse_from_npm(&info.version).is_ok_and(|version| version < deno_semver::Version::parse_from_npm("0.0.19").unwrap())
}

impl WasmModule {
  pub fn with_initializes_on_start(mut self, value: bool) -> Self {
    self.initializes_on_start = value;
    self
  }

  pub fn new(module: wasmtime::Module, engine: wasmtime::Engine) -> Result<Self> {
    Ok(Self {
      version: get_current_plugin_schema_version(&module)?,
      initializes_on_start: false,
      inner: module,
      engine,
    })
  }

  pub fn version(&self) -> PluginSchemaVersion {
    self.version
  }

  pub fn inner(&self) -> &wasmtime::Module {
    &self.inner
  }

  pub fn engine(&self) -> &wasmtime::Engine {
    &self.engine
  }

  /// Creates a store backed by the same engine as this module so that the
  /// module and the store it's instantiated in always use the same backend.
  pub fn new_store(&self, data: WasmHostState) -> Store {
    Store::new(&self.engine, data)
  }
}

/// A hash of everything wasmtime checks before loading a precompiled module:
/// the target, the cpu features the code was tuned for, the compiler settings
/// and the wasmtime version. Including it in the plugin cache key gives artifacts compiled on
/// different cpus distinct cache entries, so they can coexist in a cache
/// directory shared across machines (ex. restored from a CI cache) instead of
/// overwriting each other on every machine change.
pub fn precompile_compatibility_hash() -> u64 {
  static HASH: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
  *HASH.get_or_init(|| {
    use std::hash::Hash;
    let mut hasher = crate::utils::FastInsecureHasher::default();
    new_engine(true).precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
  })
}

// holds the engine so every module it creates shares one engine, which the
// modules and their stores must agree on
#[derive(Clone)]
pub struct WasmModuleCreator {
  engine: wasmtime::Engine,
}

impl Default for WasmModuleCreator {
  fn default() -> Self {
    Self { engine: new_engine(true) }
  }
}

impl WasmModuleCreator {
  /// A creator whose engine compiles without Cranelift's optimizations. Only
  /// used to compile; the artifacts it produces still load in the default
  /// engine (optimization level doesn't affect artifact compatibility).
  pub fn new_unoptimized() -> Self {
    Self { engine: new_engine(false) }
  }

  pub fn validate_wasm_bytes(&self, bytes: &[u8]) -> Result<()> {
    Ok(Module::validate(&self.engine, bytes)?)
  }

  pub fn create_from_wasm_bytes(&self, wasm_bytes: &[u8]) -> Result<WasmModule> {
    let module = Module::new(&self.engine, wasm_bytes)?;
    WasmModule::new(module, self.engine.clone())
  }

  /// Creates a module from the serialized native artifact produced by `compile`.
  pub fn create_from_serialized(&self, compiled_module_bytes: &[u8]) -> Result<WasmModule> {
    // SAFETY: the bytes are a cwasm artifact this same binary compiled and wrote
    // to our own cache directory; we never deserialize untrusted input. wasmtime
    // additionally rejects an artifact from an incompatible engine/CPU with an
    // error (the caller then recompiles), though that is a best-effort
    // compatibility check, not a safety boundary against tampered bytes.
    unsafe {
      match Module::deserialize(&self.engine, compiled_module_bytes) {
        Ok(module) => WasmModule::new(module, self.engine.clone()),
        Err(err) => bail!("Error deserializing compiled wasm module: {:#}", err),
      }
    }
  }

  /// Creates a module from a file holding the serialized native artifact
  /// produced by `compile`. The file is mapped into memory rather than read,
  /// so nothing is copied and only the pages that are used get loaded.
  ///
  /// The file must not change while the module is alive. The plugin cache
  /// only renames a new file over it (the same build compiled again) or
  /// removes it (once another build replaced it), which both leave the
  /// mapped file as it was.
  pub fn create_from_serialized_file(&self, file_path: &Path) -> Result<WasmModule> {
    // SAFETY: see `create_from_serialized`, and the file isn't changed in place
    unsafe {
      match Module::deserialize_file(&self.engine, file_path) {
        Ok(module) => WasmModule::new(module, self.engine.clone()),
        Err(err) => bail!("Error deserializing compiled wasm module: {:#}", err),
      }
    }
  }
}

/// The amount of wasm stack the plugin may use. wasmtime's default is 512KB,
/// which overflows on deeply nested source files (the formatter recurses over
/// the AST); 1 MiB handles them. The thread that runs the instance needs a
/// native stack at least this large (see `WASM_PLUGIN_THREAD_STACK_SIZE`).
pub const MAX_WASM_STACK_SIZE: usize = 1024 * 1024;

/// Native stack size for the (tokio blocking) thread that runs a wasm plugin
/// instance. wasmtime executes wasm on this native stack, so it must exceed
/// `MAX_WASM_STACK_SIZE` (the hard cap on wasm stack usage) with headroom for the
/// host frames around the wasm call — which is a shallow path, so 3 MiB is ample.
/// Applied via the tokio runtime's `thread_stack_size` since the instance loop
/// runs on `spawn_blocking` (tokio's default blocking-thread stack is too small
/// on some platforms, e.g. Windows). The stack is reserved address space that the
/// OS commits lazily as it's touched, so the unused headroom costs no physical
/// memory; it just needs to be large enough that wasm hits its own limit (a
/// recoverable trap) before exhausting the native stack (a crash).
pub const WASM_PLUGIN_THREAD_STACK_SIZE: usize = MAX_WASM_STACK_SIZE + 3 * 1024 * 1024;

fn new_engine(optimize: bool) -> wasmtime::Engine {
  let mut config = Config::new();
  // optimize plugins for speed. compiling without optimizations is only a
  // fallback for when an optimized compile stalls (see compile_worker.rs)
  config.cranelift_opt_level(if optimize { wasmtime::OptLevel::Speed } else { wasmtime::OptLevel::None });
  #[cfg(use_pulley)]
  {
    // no native Cranelift backend (or signal-based traps) for this target, so
    // compile to Pulley bytecode and interpret it. every target that sets
    // `use_pulley` is 64-bit, so use the 64-bit Pulley target for the matching
    // endianness.
    let pulley_target = if cfg!(target_endian = "big") { "pulley64be" } else { "pulley64" };
    config.target(pulley_target).expect("failed to set pulley target");
  }
  config.table_lazy_init(false);
  #[cfg(not(windows))]
  config.native_unwind_info(false);
  config.max_wasm_stack(MAX_WASM_STACK_SIZE);
  Engine::new(&config).expect("failed to create wasmtime engine")
}

// a mapped file can't be replaced on Windows, which is why modules are only
// mapped on unix
#[cfg(all(test, unix))]
mod test {
  use super::*;
  use crate::test_helpers::WASM_PLUGIN_BYTES;

  fn plugin_name(module: &WasmModule) -> String {
    let linker = super::super::create_identity_import_object(module.version(), module.engine()).unwrap();
    let mut store = module.new_store(WasmHostState::Empty);
    let instance = load_instance(&mut store, module, &linker).unwrap();
    super::super::create_wasm_plugin_instance(store, instance).unwrap().plugin_info().unwrap().name
  }

  #[test]
  #[allow(clippy::disallowed_methods)] // the module is mapped from a real file
  fn creates_a_module_from_a_serialized_file() {
    let bytes = super::super::compile(WASM_PLUGIN_BYTES).unwrap().bytes;
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("plugin.cwasm");
    std::fs::write(&file_path, &bytes).unwrap();
    let wasm_module_creator = WasmModuleCreator::default();
    let module = wasm_module_creator.create_from_serialized_file(&file_path).unwrap();

    // replacing the file the way the plugin cache does leaves the module as it was
    let temp_file_path = dir.path().join("plugin.cwasm.tmp");
    std::fs::write(&temp_file_path, b"not a module").unwrap();
    std::fs::rename(&temp_file_path, &file_path).unwrap();
    assert_eq!(plugin_name(&module), "test-plugin");
    assert!(wasm_module_creator.create_from_serialized_file(&file_path).is_err());
  }
}

#[cfg(test)]
mod tests {
  use wasmtime::Val;

  use super::super::instance::create_identity_import_object;
  use super::*;

  #[test]
  fn initializes_wasi_reactor() {
    let (mut store, instance) = load(
      r#"(import "wasi_snapshot_preview1" "sock_shutdown" (func (param i32 i32) (result i32)))
         (global $count (mut i32) (i32.const 0))
         (func (export "_initialize") (global.set $count (i32.add (global.get $count) (i32.const 1))))
         (func (export "get_count") (result i32) (global.get $count))"#,
    )
    .unwrap();
    let mut results = [Val::I32(0)];
    let get_count = instance.get_function(&mut store, "get_count").unwrap();
    get_count.call(&mut store, &[], &mut results).unwrap();
    assert_eq!(results[0].unwrap_i32(), 1);
  }

  #[test]
  fn skips_initialize_when_module_initializes_on_start() {
    // traps when initialized a second time
    let body = r#"(global $count (mut i32) (i32.const 0))
         (func $init (export "_initialize")
           (if (global.get $count) (then unreachable))
           (global.set $count (i32.const 1)))
         (start $init)"#;
    let err = load(body).err().unwrap();
    assert!(format!("{:#}", err).starts_with("Error initializing module: "), "{:#}", err);
    assert!(load_with(body, |module| module.with_initializes_on_start(true)).is_ok());
  }

  #[test]
  fn gofumpt_initializes_on_start() {
    let run = |name: &str, version: &str| {
      plugin_initializes_on_start(&kprint_plugin_types::PluginInfo {
        name: name.to_string(),
        version: version.to_string(),
        config_key: String::new(),
        config_relative_path_bases: Default::default(),
        config_files: None,
        config_schema_paths: vec![],
        config_local_schemas_use_root: false,
        config_reference_paths: vec![],
        help_url: String::new(),
        config_schema_url: String::new(),
        update_url: None,
        supports_range_formatting: None,
        allow_plugin_override_config: None,
      })
    };
    assert!(run("dprint-plugin-gofumpt", "0.0.1"));
    assert!(run("dprint-plugin-gofumpt", "0.0.18"));
    assert!(!run("dprint-plugin-gofumpt", "0.0.19"));
    assert!(!run("dprint-plugin-gofumpt", "0.1.0"));
    assert!(!run("dprint-plugin-gofumpt", "invalid"));
    assert!(!run("dprint-plugin-other", "0.0.18"));
  }

  #[test]
  fn errors_when_initializing_fails() {
    let err = load(
      r#"(import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
         (func (export "_initialize") (call $proc_exit (i32.const 1)))"#,
    )
    .err()
    .unwrap();
    let text = format!("{:#}", err);
    assert!(text.starts_with("Error initializing module: "), "{}", text);
    assert!(text.contains("The plugin attempted to exit with code 1."), "{}", text);
  }

  #[test]
  fn errors_for_wasi_command() {
    let err = load(r#"(func (export "_start"))"#).err().unwrap();
    assert_eq!(
      format!("{:#}", err),
      "WASI plugins must be built as reactors exporting _initialize, not commands exporting _start."
    );
    // ok when it has both
    assert!(load(r#"(func (export "_start")) (func (export "_initialize"))"#).is_ok());
  }

  #[test]
  fn runs_module_with_exception_handling() {
    // NativeAOT-LLVM plugins built with WasmEnableExceptionHandling throw and catch with exnref
    let (mut store, instance) = load(
      r#"(tag $e (param i32))
         (func $thrower (param i32) (throw $e (local.get 0)))
         (func (export "rethrows") (param i32) (result i32)
           (block $outer (result i32)
             (try_table (catch $e $outer)
               (block $caught (result i32 exnref)
                 (try_table (catch_ref $e $caught)
                   (call $thrower (local.get 0)))
                 (unreachable))
               (throw_ref))
             (i32.const -1)))"#,
    )
    .unwrap();
    let mut results = [Val::I32(0)];
    let rethrows = instance.get_function(&mut store, "rethrows").unwrap();
    rethrows.call(&mut store, &[Val::I32(7)], &mut results).unwrap();
    assert_eq!(results[0].unwrap_i32(), 7);
  }

  #[test]
  fn errors_for_unknown_import() {
    let err = load(r#"(import "dprint" "host_unknown" (func))"#).err().unwrap();
    let text = format!("{:#}", err);
    assert!(text.starts_with("Error instantiating module: "), "{}", text);
    assert!(text.contains("host_unknown"), "{}", text);
  }

  fn load(body: &str) -> Result<(Store, WasmInstance)> {
    load_with(body, |module| module)
  }

  fn load_with(body: &str, map_module: impl FnOnce(WasmModule) -> WasmModule) -> Result<(Store, WasmInstance)> {
    let wasm = wat::parse_str(format!(
      r#"(module {} (func (export "dprint_plugin_version_4") (result i32) (i32.const 4)))"#,
      body
    ))?;
    let module = map_module(WasmModuleCreator::default().create_from_wasm_bytes(&wasm)?);
    let linker = create_identity_import_object(module.version(), module.engine())?;
    let mut store = module.new_store(WasmHostState::Empty);
    let instance = load_instance(&mut store, &module, &linker)?;
    Ok((store, instance))
  }
}
