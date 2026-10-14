#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControllerPacketKind {
    Event,
    Acl,
    Iso,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControllerPacket {
    Event(Vec<u8>),
    Acl(Vec<u8>),
    Iso(Vec<u8>),
}

impl ControllerPacket {
    pub fn kind(&self) -> ControllerPacketKind {
        match self {
            Self::Event(_) => ControllerPacketKind::Event,
            Self::Acl(_) => ControllerPacketKind::Acl,
            Self::Iso(_) => ControllerPacketKind::Iso,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Event(bytes) | Self::Acl(bytes) | Self::Iso(bytes) => bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPacket {
    Command(Vec<u8>),
    Acl(Vec<u8>),
    Iso(Vec<u8>),
}
