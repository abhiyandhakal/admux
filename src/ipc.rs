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
    ReloadConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandResponse {
    HelloAck { version: ProtocolVersion },
    SessionCreated { session: String, pane_id: u64 },
    Attached { session: String, preview: String },
    SessionList { sessions: Vec<String> },
    SessionKilled { session: String },
    KeysSent,
    ConfigReloaded,
    Error { message: String },
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
}
