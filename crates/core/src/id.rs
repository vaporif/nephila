use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectiveId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterruptId(pub uuid::Uuid);

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

impl Default for CheckpointId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl CheckpointId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for InterruptId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl InterruptId {
    pub fn new() -> Self {
        Self::default()
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

impl_uuid_id_display!(AgentId, EntryId, ObjectiveId, CheckpointId, InterruptId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_id_new_is_unique() {
        let a = CheckpointId::new();
        let b = CheckpointId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn interrupt_id_new_is_unique() {
        let a = InterruptId::new();
        let b = InterruptId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn checkpoint_id_display_shows_hex_prefix() {
        let id = CheckpointId::new();
        let s = format!("{id}");
        assert_eq!(s.len(), 8);
    }

    #[test]
    fn interrupt_id_display_shows_hex_prefix() {
        let id = InterruptId::new();
        let s = format!("{id}");
        assert_eq!(s.len(), 8);
    }
}
