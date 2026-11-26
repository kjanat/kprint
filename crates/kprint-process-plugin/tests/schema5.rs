use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use kprint_async_runtime::FutureExt;
use kprint_plugin_types::FormatConfigId;
use kprint_plugin_types::NullCancellationToken;
use kprint_process_plugin::ProcessPluginCommunicator;
use kprint_process_plugin::ProcessPluginCommunicatorFormatRequest;

#[test]
fn accepts_independent_schema_five_process_and_its_host_format() {
  let directory = std::env::temp_dir().join(format!("kprint-schema5-{}", std::process::id()));
  std::fs::create_dir_all(&directory).unwrap();
  let executable = directory.join(if cfg!(windows) { "schema5.exe" } else { "schema5" });
  let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/schema5.rs");
  let status = std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
    .arg("--edition=2021")
    .arg(source)
    .arg("-o")
    .arg(&executable)
    .status()
    .unwrap();
  assert!(status.success());
  let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
  runtime.block_on(async {
    for name in ["schema4", "schema7"] {
      let path = directory.join(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() });
      std::fs::copy(&executable, &path).unwrap();
      let result = ProcessPluginCommunicator::new(&path, |_| {}).await;
      assert!(result.is_err(), "unsupported version {name} was accepted");
    }
    let communicator = ProcessPluginCommunicator::new(&executable, |message| panic!("{message}")).await.unwrap();
    let result = communicator
      .format_text(ProcessPluginCommunicatorFormatRequest {
        file_path: "outer.txt".into(),
        file_bytes: b"original".to_vec(),
        range: None,
        config_id: FormatConfigId::from_raw(1),
        override_config: Default::default(),
        on_host_format: Rc::new(|request| {
          async move {
            assert_eq!(request.file_path, PathBuf::from("nested.txt"));
            assert_eq!(request.file_bytes, b"original");
            Ok(Some(b"formatted".to_vec()))
          }
          .boxed_local()
        }),
        token: Arc::new(NullCancellationToken),
      })
      .await
      .unwrap();
    assert_eq!(result, Some(b"formatted".to_vec()));
    communicator.shutdown().await;
  });
  std::fs::remove_dir_all(directory).unwrap();
}
