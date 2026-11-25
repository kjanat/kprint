use std::io::ErrorKind;
use std::io::Read;
use std::io::Result;
use std::io::Write;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use kprint_communication::Message;
use kprint_plugin_types::ConfigChange;
use kprint_plugin_types::FormatConfigId;
use kprint_plugin_types::FormatRange;

use kprint_communication::MessageReader;
use kprint_communication::MessageWriter;

pub type MessageId = u32;

mod message_ids {
  use super::MessageId;

  pub const SUCCESS_ID: MessageId = 0;
  pub const DATA_RESPONSE_ID: MessageId = 1;
  pub const ERROR_ID: MessageId = 2;
  pub const CLOSE_ID: MessageId = 3;
  pub const IS_ALIVE_ID: MessageId = 4;
  pub const GET_PLUGIN_INFO_ID: MessageId = 5;
  pub const GET_LICENSE_TEXT_ID: MessageId = 6;
  pub const REGISTER_CONFIG_ID: MessageId = 7;
  pub const RELEASE_CONFIG_ID: MessageId = 8;
  pub const GET_CONFIG_DIAGNOSTICS_ID: MessageId = 9;
  pub const GET_FILE_MATCHING_INFO_ID: MessageId = 10;
  pub const GET_RESOLVED_CONFIG_ID: MessageId = 11;
  pub const CHECK_CONFIG_UPDATES_ID: MessageId = 12;
  pub const FORMAT_ID: MessageId = 13;
  pub const FORMAT_RESPONSE_ID: MessageId = 14;
  pub const CANCEL_FORMAT_ID: MessageId = 15;
  pub const HOST_FORMAT_ID: MessageId = 16;
  pub const HOST_FORMAT_LANGUAGE_ID: MessageId = 17;
}

#[derive(Debug)]
pub struct ProcessPluginMessage {
  pub id: MessageId,
  pub body: MessageBody,
}

impl ProcessPluginMessage {
  pub fn read<TRead: Read + Unpin>(reader: &mut MessageReader<TRead>) -> Result<ProcessPluginMessage> {
    let id = reader.read_u32()?;
    let message_kind = reader.read_u32()?;
    let body = match message_kind {
      message_ids::SUCCESS_ID => MessageBody::Success(reader.read_u32()?),
      message_ids::DATA_RESPONSE_ID => {
        let message_id = reader.read_u32()?;
        let data = reader.read_sized_bytes()?;
        MessageBody::DataResponse(ResponseBody { message_id, data })
      }
      message_ids::ERROR_ID => {
        let message_id = reader.read_u32()?;
        let data = reader.read_sized_bytes()?;
        MessageBody::Error(ResponseBody { message_id, data })
      }
      message_ids::CLOSE_ID => MessageBody::Close,
      message_ids::IS_ALIVE_ID => MessageBody::IsAlive,
      message_ids::GET_PLUGIN_INFO_ID => MessageBody::GetPluginInfo,
      message_ids::GET_LICENSE_TEXT_ID => MessageBody::GetLicenseText,
      message_ids::REGISTER_CONFIG_ID => {
        let config_id = FormatConfigId::from_raw(reader.read_u32()?);
        let global_config = reader.read_sized_bytes()?;
        let plugin_config = reader.read_sized_bytes()?;
        MessageBody::RegisterConfig(RegisterConfigMessageBody {
          config_id,
          global_config,
          plugin_config,
        })
      }
      message_ids::RELEASE_CONFIG_ID => MessageBody::ReleaseConfig(FormatConfigId::from_raw(reader.read_u32()?)),
      message_ids::GET_CONFIG_DIAGNOSTICS_ID => MessageBody::GetConfigDiagnostics(FormatConfigId::from_raw(reader.read_u32()?)),
      message_ids::GET_FILE_MATCHING_INFO_ID => MessageBody::GetFileMatchingInfo(FormatConfigId::from_raw(reader.read_u32()?)),
      message_ids::GET_RESOLVED_CONFIG_ID => MessageBody::GetResolvedConfig(FormatConfigId::from_raw(reader.read_u32()?)),
      message_ids::CHECK_CONFIG_UPDATES_ID => {
        let body_bytes = reader.read_sized_bytes()?;
        MessageBody::CheckConfigUpdates(body_bytes)
      }
      message_ids::FORMAT_ID => {
        let file_path = reader.read_sized_bytes()?;
        let start_byte_index = reader.read_u32()?;
        let end_byte_index = reader.read_u32()?;
        let config_id = FormatConfigId::from_raw(reader.read_u32()?);
        let override_config = reader.read_sized_bytes()?;
        let file_text = reader.read_sized_bytes()?;
        MessageBody::Format(FormatMessageBody {
          file_path: PathBuf::from(String::from_utf8_lossy(&file_path).to_string()),
          range: if start_byte_index == 0 && end_byte_index == file_text.len() as u32 {
            None
          } else {
            Some(std::ops::Range {
              start: start_byte_index as usize,
              end: end_byte_index as usize,
            })
          },
          config_id,
          file_bytes: file_text,
          override_config,
        })
      }
      message_ids::FORMAT_RESPONSE_ID => {
        let message_id = reader.read_u32()?;
        let response_kind = reader.read_u32()?;
        let data = match response_kind {
          0 => None,
          1 => Some(reader.read_sized_bytes()?),
          _ => {
            return Err(std::io::Error::new(
              ErrorKind::InvalidData,
              format!("Unknown format response kind: {}", response_kind),
            ));
          }
        };
        MessageBody::FormatResponse(ResponseBody { message_id, data })
      }
      message_ids::CANCEL_FORMAT_ID => MessageBody::CancelFormat(reader.read_u32()?),
      message_ids::HOST_FORMAT_ID => {
        let original_message_id = reader.read_u32()?;
        let file_path = reader.read_sized_bytes()?;
        let start_byte_index = reader.read_u32()?;
        let end_byte_index = reader.read_u32()?;
        let override_config = reader.read_sized_bytes()?;
        let file_text = reader.read_sized_bytes()?;
        MessageBody::HostFormat(HostFormatMessageBody {
          original_message_id,
          file_path: PathBuf::from(String::from_utf8_lossy(&file_path).to_string()),
          range: if start_byte_index == 0 && end_byte_index == file_text.len() as u32 {
            None
          } else {
            Some(std::ops::Range {
              start: start_byte_index as usize,
              end: end_byte_index as usize,
            })
          },
          file_text,
          override_config,
        })
      }
      message_ids::HOST_FORMAT_LANGUAGE_ID => {
        let original_message_id = reader.read_u32()?;
        let length = reader.read_u32()? as usize;
        if length == 0 || length > 1024 {
          return Err(std::io::Error::new(ErrorKind::InvalidData, "Invalid language label length"));
        }
        let language = String::from_utf8(reader.read_bytes(length)?).map_err(|err| std::io::Error::new(ErrorKind::InvalidData, err))?;
        let range = match reader.read_u32()? {
          0 => None,
          1 => Some(reader.read_u32()? as usize..reader.read_u32()? as usize),
          _ => return Err(std::io::Error::new(ErrorKind::InvalidData, "Invalid range kind")),
        };
        let override_config = reader.read_sized_bytes()?;
        let file_text = reader.read_sized_bytes()?;
        if range.as_ref().is_some_and(|range| range.start > range.end || range.end > file_text.len()) {
          return Err(std::io::Error::new(ErrorKind::InvalidData, "Invalid format range"));
        }
        MessageBody::HostFormatLanguage(HostFormatLanguageMessageBody {
          original_message_id,
          language,
          range,
          override_config,
          file_text,
        })
      }
      _ => {
        // don't read success bytes... receiving this means that
        // the plugin should exit the process after returning an
        // error or panic
        return Ok(ProcessPluginMessage {
          id,
          body: MessageBody::Unknown(message_kind),
        });
      }
    };
    reader.read_success_bytes()?;
    Ok(ProcessPluginMessage { id, body })
  }
}

impl Message for ProcessPluginMessage {
  fn write<TWrite: Write + Unpin>(&self, writer: &mut MessageWriter<TWrite>) -> Result<()> {
    writer.send_u32(self.id)?;
    match &self.body {
      MessageBody::Success(message_id) => {
        writer.send_u32(message_ids::SUCCESS_ID)?;
        writer.send_u32(*message_id)?;
      }
      MessageBody::DataResponse(response) => {
        writer.send_u32(message_ids::DATA_RESPONSE_ID)?;
        writer.send_u32(response.message_id)?;
        writer.send_sized_bytes(&response.data)?;
      }
      MessageBody::Error(response) => {
        writer.send_u32(message_ids::ERROR_ID)?;
        writer.send_u32(response.message_id)?;
        writer.send_sized_bytes(&response.data)?;
      }
      MessageBody::Close => {
        writer.send_u32(message_ids::CLOSE_ID)?;
      }
      MessageBody::IsAlive => {
        writer.send_u32(message_ids::IS_ALIVE_ID)?;
      }
      MessageBody::GetPluginInfo => {
        writer.send_u32(message_ids::GET_PLUGIN_INFO_ID)?;
      }
      MessageBody::GetLicenseText => {
        writer.send_u32(message_ids::GET_LICENSE_TEXT_ID)?;
      }
      MessageBody::RegisterConfig(body) => {
        writer.send_u32(message_ids::REGISTER_CONFIG_ID)?;
        writer.send_u32(body.config_id.as_raw())?;
        writer.send_sized_bytes(&body.global_config)?;
        writer.send_sized_bytes(&body.plugin_config)?;
      }
      MessageBody::ReleaseConfig(config_id) => {
        writer.send_u32(message_ids::RELEASE_CONFIG_ID)?;
        writer.send_u32(config_id.as_raw())?;
      }
      MessageBody::GetConfigDiagnostics(config_id) => {
        writer.send_u32(message_ids::GET_CONFIG_DIAGNOSTICS_ID)?;
        writer.send_u32(config_id.as_raw())?;
      }
      MessageBody::GetFileMatchingInfo(config_id) => {
        writer.send_u32(message_ids::GET_FILE_MATCHING_INFO_ID)?;
        writer.send_u32(config_id.as_raw())?;
      }
      MessageBody::GetResolvedConfig(config_id) => {
        writer.send_u32(message_ids::GET_RESOLVED_CONFIG_ID)?;
        writer.send_u32(config_id.as_raw())?;
      }
      MessageBody::CheckConfigUpdates(body_bytes) => {
        writer.send_u32(message_ids::CHECK_CONFIG_UPDATES_ID)?;
        writer.send_sized_bytes(body_bytes)?;
      }
      MessageBody::Format(body) => {
        writer.send_u32(message_ids::FORMAT_ID)?;
        writer.send_sized_bytes(body.file_path.to_string_lossy().as_bytes())?;
        writer.send_u32(body.range.as_ref().map(|r| r.start).unwrap_or(0) as u32)?;
        writer.send_u32(body.range.as_ref().map(|r| r.end).unwrap_or(body.file_bytes.len()) as u32)?;
        writer.send_u32(body.config_id.as_raw())?;
        writer.send_sized_bytes(&body.override_config)?;
        writer.send_sized_bytes(&body.file_bytes)?;
      }
      MessageBody::FormatResponse(response) => {
        writer.send_u32(message_ids::FORMAT_RESPONSE_ID)?;
        writer.send_u32(response.message_id)?;
        match &response.data {
          None => {
            writer.send_u32(0)?;
          }
          Some(data) => {
            writer.send_u32(1)?;
            writer.send_sized_bytes(data)?;
          }
        }
      }
      MessageBody::CancelFormat(message_id) => {
        writer.send_u32(message_ids::CANCEL_FORMAT_ID)?;
        writer.send_u32(*message_id)?;
      }
      MessageBody::HostFormat(body) => {
        writer.send_u32(message_ids::HOST_FORMAT_ID)?;
        writer.send_u32(body.original_message_id)?;
        writer.send_sized_bytes(body.file_path.to_string_lossy().as_bytes())?;
        writer.send_u32(body.range.as_ref().map(|r| r.start).unwrap_or(0) as u32)?;
        writer.send_u32(body.range.as_ref().map(|r| r.end).unwrap_or(body.file_text.len()) as u32)?;
        writer.send_sized_bytes(&body.override_config)?;
        writer.send_sized_bytes(&body.file_text)?;
      }
      MessageBody::HostFormatLanguage(body) => {
        if body.language.is_empty() || body.language.len() > 1024 {
          return Err(std::io::Error::new(ErrorKind::InvalidInput, "Invalid language label length"));
        }
        writer.send_u32(message_ids::HOST_FORMAT_LANGUAGE_ID)?;
        writer.send_u32(body.original_message_id)?;
        writer.send_sized_bytes(body.language.as_bytes())?;
        match &body.range {
          None => writer.send_u32(0)?,
          Some(range) => {
            if range.start > range.end || range.end > body.file_text.len() {
              return Err(std::io::Error::new(ErrorKind::InvalidInput, "Invalid format range"));
            }
            let start = u32::try_from(range.start).map_err(|_| std::io::Error::new(ErrorKind::InvalidInput, "Format range exceeds u32"))?;
            let end = u32::try_from(range.end).map_err(|_| std::io::Error::new(ErrorKind::InvalidInput, "Format range exceeds u32"))?;
            writer.send_u32(1)?;
            writer.send_u32(start)?;
            writer.send_u32(end)?;
          }
        }
        writer.send_sized_bytes(&body.override_config)?;
        writer.send_sized_bytes(&body.file_text)?;
      }
      MessageBody::Unknown(_) => unreachable!(), // should never be written
    }
    writer.send_success_bytes()?;
    Ok(())
  }
}

#[derive(Debug)]
pub enum MessageBody {
  Success(MessageId),
  DataResponse(ResponseBody<Vec<u8>>),
  Error(ResponseBody<Vec<u8>>),
  Close,
  IsAlive,
  GetPluginInfo,
  GetLicenseText,
  RegisterConfig(RegisterConfigMessageBody),
  ReleaseConfig(FormatConfigId),
  GetConfigDiagnostics(FormatConfigId),
  GetFileMatchingInfo(FormatConfigId),
  GetResolvedConfig(FormatConfigId),
  CheckConfigUpdates(Vec<u8>),
  Format(FormatMessageBody),
  FormatResponse(ResponseBody<Option<Vec<u8>>>),
  CancelFormat(MessageId),
  HostFormat(HostFormatMessageBody),
  HostFormatLanguage(HostFormatLanguageMessageBody),
  /// If encountered, process plugin should panic and
  /// the CLI should kill the process plugin.
  Unknown(u32),
}

#[derive(Debug)]
pub struct ResponseBody<T: std::fmt::Debug> {
  pub message_id: MessageId,
  pub data: T,
}

#[derive(Debug)]
pub struct RegisterConfigMessageBody {
  pub config_id: FormatConfigId,
  pub global_config: Vec<u8>,
  pub plugin_config: Vec<u8>,
}

pub type CheckConfigUpdatesMessageBody = kprint_plugin_types::CheckConfigUpdatesMessage;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckConfigUpdatesResponseBody {
  pub changes: Vec<ConfigChange>,
}

#[derive(Debug)]
pub struct FormatMessageBody {
  pub file_path: PathBuf,
  pub range: FormatRange,
  pub config_id: FormatConfigId,
  pub override_config: Vec<u8>,
  pub file_bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct HostFormatMessageBody {
  pub original_message_id: MessageId,
  pub file_path: PathBuf,
  pub range: FormatRange,
  pub override_config: Vec<u8>,
  pub file_text: Vec<u8>,
}

#[derive(Debug)]
pub struct HostFormatLanguageMessageBody {
  pub original_message_id: MessageId,
  pub language: String,
  pub range: FormatRange,
  pub override_config: Vec<u8>,
  pub file_text: Vec<u8>,
}

#[cfg(test)]
mod language_tests {
  use super::*;

  #[test]
  fn schema_five_host_format_wire_layout_is_unchanged() {
    let message = ProcessPluginMessage {
      id: 1,
      body: MessageBody::HostFormat(HostFormatMessageBody {
        original_message_id: 2,
        file_path: "x.js".into(),
        range: Some(1..3),
        override_config: b"{}".to_vec(),
        file_text: b"text".to_vec(),
      }),
    };
    let mut actual = Vec::new();
    message.write(&mut MessageWriter::new(&mut actual)).unwrap();
    let mut expected = Vec::new();
    for value in [1u32, 16, 2, 4] {
      expected.extend_from_slice(&value.to_be_bytes());
    }
    expected.extend_from_slice(b"x.js");
    for value in [1u32, 3, 2] {
      expected.extend_from_slice(&value.to_be_bytes());
    }
    expected.extend_from_slice(b"{}");
    expected.extend_from_slice(&4u32.to_be_bytes());
    expected.extend_from_slice(b"text");
    expected.extend_from_slice(&[255; 4]);
    assert_eq!(actual, expected);
    assert!(matches!(
      ProcessPluginMessage::read(&mut MessageReader::new(expected.as_slice())).unwrap().body,
      MessageBody::HostFormat(_)
    ));
  }

  #[test]
  fn language_message_round_trips_and_preserves_context() {
    let message = ProcessPluginMessage {
      id: 42,
      body: MessageBody::HostFormatLanguage(HostFormatLanguageMessageBody {
        original_message_id: 7,
        language: "TypeScript".into(),
        range: Some(1..3),
        override_config: br#"{"indentWidth":4}"#.to_vec(),
        file_text: b"text".to_vec(),
      }),
    };
    let mut bytes = Vec::new();
    message.write(&mut MessageWriter::new(&mut bytes)).unwrap();
    let decoded = ProcessPluginMessage::read(&mut MessageReader::new(bytes.as_slice())).unwrap();
    let MessageBody::HostFormatLanguage(body) = decoded.body else {
      panic!("wrong message")
    };
    assert_eq!(decoded.id, 42);
    assert_eq!(body.original_message_id, 7);
    assert_eq!(body.language, "TypeScript");
    assert_eq!(body.range, Some(1..3));
    assert_eq!(body.override_config, br#"{"indentWidth":4}"#);
    assert_eq!(body.file_text, b"text");
  }

  #[test]
  fn malformed_language_labels_are_rejected_before_allocation() {
    for (length, label) in [(0u32, &b""[..]), (1025, &b""[..]), (1, &b"\xff"[..])] {
      let mut bytes = Vec::new();
      for value in [1u32, 17, 2, length] {
        bytes.extend_from_slice(&value.to_be_bytes());
      }
      bytes.extend_from_slice(label);
      let error = ProcessPluginMessage::read(&mut MessageReader::new(bytes.as_slice())).unwrap_err();
      assert_eq!(error.kind(), ErrorKind::InvalidData);
    }
  }

  #[test]
  fn language_range_is_validated_and_full_range_is_distinct_from_none() {
    for range in [None, Some(0..4)] {
      let message = ProcessPluginMessage {
        id: 1,
        body: MessageBody::HostFormatLanguage(HostFormatLanguageMessageBody {
          original_message_id: 2,
          language: "js".into(),
          range: range.clone(),
          override_config: vec![],
          file_text: b"text".to_vec(),
        }),
      };
      let mut bytes = Vec::new();
      message.write(&mut MessageWriter::new(&mut bytes)).unwrap();
      let decoded = ProcessPluginMessage::read(&mut MessageReader::new(bytes.as_slice())).unwrap();
      let MessageBody::HostFormatLanguage(body) = decoded.body else {
        panic!("wrong message")
      };
      assert_eq!(body.range, range);
    }
    for range in [3..2, 0..5] {
      let message = ProcessPluginMessage {
        id: 1,
        body: MessageBody::HostFormatLanguage(HostFormatLanguageMessageBody {
          original_message_id: 2,
          language: "js".into(),
          range: Some(range),
          override_config: vec![],
          file_text: b"text".to_vec(),
        }),
      };
      assert_eq!(message.write(&mut MessageWriter::new(Vec::new())).unwrap_err().kind(), ErrorKind::InvalidInput);
    }
    let mut bytes = Vec::new();
    for value in [1u32, 17, 2, 2] {
      bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(b"js");
    for value in [1u32, 3, 2, 0, 4] {
      bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(b"text");
    bytes.extend_from_slice(&[255; 4]);
    assert_eq!(
      ProcessPluginMessage::read(&mut MessageReader::new(bytes.as_slice())).unwrap_err().kind(),
      ErrorKind::InvalidData
    );
  }
}
