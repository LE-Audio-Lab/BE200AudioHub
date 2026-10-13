use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressKind {
    Public,
    Random,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PeerAddress {
    pub value: String,
    pub kind: AddressKind,
}

impl PeerAddress {
    pub fn new(value: impl Into<String>, kind: AddressKind) -> Self {
        Self {
            value: value.into(),
            kind,
        }
    }
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({:?})", self.value, self.kind)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MemberIdentity(pub String);

impl MemberIdentity {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SetHandle(pub String);

impl SetHandle {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetDescriptor {
    pub handle: SetHandle,
    /// Total number of coordinated-set members, including the primary.
    pub size: u8,
}

impl SetDescriptor {
    pub fn new(handle: SetHandle, size: u8) -> Self {
        Self { handle, size }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedSetMember {
    /// Address exactly as supplied by the scanner/connection manager.
    pub advertised_address: PeerAddress,
    pub set: SetDescriptor,
    pub rsi: [u8; 6],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BondRole {
    Primary,
    SetMember,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondRequest {
    pub role: BondRole,
    pub address: PeerAddress,
    pub expected_set: Option<SetHandle>,
}

impl BondRequest {
    pub fn primary(address: PeerAddress) -> Self {
        Self {
            role: BondRole::Primary,
            address,
            expected_set: None,
        }
    }

    pub fn set_member(expected: &ExpectedSetMember) -> Self {
        Self {
            role: BondRole::SetMember,
            address: expected.advertised_address.clone(),
            expected_set: Some(expected.set.handle.clone()),
        }
    }
}
