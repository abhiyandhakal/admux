use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion(pub u16);

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandRequest {
    Hello {
        version: ProtocolVersion,
    },
    NewSession {
        name: Option<String>,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    Attach {
        session: Option<String>,
    },
    ListSessions,
    KillSession {
        session: String,
    },
    SendKeys {
        target: String,
        keys: Vec<String>,
    },
    MouseScroll {
        session: String,
        row: u16,
        col: u16,
        direction: ScrollDirection,
    },
    CopySelection {
        session: String,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    },
    Resize {
        session: String,
        rows: u16,
        cols: u16,
    },
    ReloadConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandResponse {
    HelloAck {
        version: ProtocolVersion,
    },
    SessionCreated {
        session: String,
        pane_id: u64,
    },
    Attached {
        session: String,
        preview: String,
        #[serde(default)]
        formatted_preview: String,
        #[serde(default)]
        formatted_cursor: String,
    },
    SessionList {
        sessions: Vec<String>,
    },
    SessionKilled {
        session: String,
    },
    KeysSent,
    SelectionCopied {
        text: String,
    },
    Scrolled,
    Resized,
    ConfigReloaded,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_roundtrip_preserves_request() {
        let request = CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: Some(PathBuf::from("/tmp")),
            command: vec!["bash".into()],
        };
        let encoded = serde_json::to_vec(&request).expect("encode request");
        let decoded: CommandRequest = serde_json::from_slice(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn protocol_version_is_non_zero() {
        assert!(CURRENT_PROTOCOL_VERSION.0 > 0);
    }

    #[test]
    fn attached_response_accepts_older_payload_without_formatted_preview() {
        let payload = br#"{"Attached":{"session":"work","preview":"plain output"}}"#;
        let decoded: CommandResponse =
            serde_json::from_slice(payload).expect("decode legacy attached response");

        assert_eq!(
            decoded,
            CommandResponse::Attached {
                session: "work".into(),
                preview: "plain output".into(),
                formatted_preview: String::new(),
                formatted_cursor: String::new(),
            }
        );
    }
}
