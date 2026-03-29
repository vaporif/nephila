use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectiveId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckpointVersion(pub u32);

impl Default for AgentId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl AgentId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for EntryId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl EntryId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for ObjectiveId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl ObjectiveId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CheckpointVersion {
    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("checkpoint version overflow"))
    }
}

macro_rules! impl_uuid_id_display {
    ($($ty:ident),+) => {$(
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut buf = [0u8; uuid::fmt::Simple::LENGTH];
                let hex = self.0.simple().encode_lower(&mut buf);
                f.write_str(&hex[..8])
            }
        }
    )+};
}

impl_uuid_id_display!(AgentId, EntryId, ObjectiveId);

impl fmt::Display for CheckpointVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}
