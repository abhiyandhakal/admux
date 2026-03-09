#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolVersion(pub u16);

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);
