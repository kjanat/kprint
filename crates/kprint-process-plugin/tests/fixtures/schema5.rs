// Frozen, independently implemented subset of the schema 5 process protocol.
use std::io::{self, Read, Write};

fn u32_read(r: &mut impl Read) -> u32 {
  let mut b = [0; 4];
  r.read_exact(&mut b).unwrap();
  u32::from_be_bytes(b)
}
fn bytes_read(r: &mut impl Read) -> Vec<u8> {
  let len = u32_read(r) as usize;
  let mut bytes = vec![0; len];
  r.read_exact(&mut bytes).unwrap();
  bytes
}
fn word(w: &mut impl Write, value: u32) {
  w.write_all(&value.to_be_bytes()).unwrap();
}
fn bytes(w: &mut impl Write, value: &[u8]) {
  word(w, value.len() as u32);
  w.write_all(value).unwrap();
}
fn finish(w: &mut impl Write) {
  w.write_all(&[255; 4]).unwrap();
  w.flush().unwrap();
}
fn main() {
  let mut input = io::stdin().lock();
  let mut output = io::stdout().lock();
  assert_eq!(u32_read(&mut input), 0);
  word(&mut output, 0);
  let executable = std::env::current_exe().unwrap();
  let name = executable.file_stem().unwrap().to_string_lossy();
  let version = if name == "schema4" {
    4
  } else if name == "schema7" {
    7
  } else {
    5
  };
  word(&mut output, version);
  output.flush().unwrap();
  if version != 5 {
    return;
  }
  loop {
    let id = u32_read(&mut input);
    match u32_read(&mut input) {
      3 => {
        assert_eq!(u32_read(&mut input), u32::MAX);
        word(&mut output, 99);
        word(&mut output, 0);
        word(&mut output, id);
        finish(&mut output);
        break;
      }
      13 => {
        let _path = bytes_read(&mut input);
        let _start = u32_read(&mut input);
        let _end = u32_read(&mut input);
        let _config_id = u32_read(&mut input);
        let _config = bytes_read(&mut input);
        let text = bytes_read(&mut input);
        assert_eq!(u32_read(&mut input), u32::MAX);
        word(&mut output, 77);
        word(&mut output, 16);
        word(&mut output, id);
        bytes(&mut output, b"nested.txt");
        word(&mut output, 0);
        word(&mut output, text.len() as u32);
        bytes(&mut output, b"{}");
        bytes(&mut output, &text);
        finish(&mut output);
        let _response_id = u32_read(&mut input);
        assert_eq!(u32_read(&mut input), 14);
        assert_eq!(u32_read(&mut input), 77);
        assert_eq!(u32_read(&mut input), 1);
        let formatted = bytes_read(&mut input);
        assert_eq!(u32_read(&mut input), u32::MAX);
        word(&mut output, 78);
        word(&mut output, 14);
        word(&mut output, id);
        word(&mut output, 1);
        bytes(&mut output, &formatted);
        finish(&mut output);
      }
      kind => panic!("unexpected message {kind}"),
    }
  }
}
