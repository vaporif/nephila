use crate::checkpoint::{ChannelEntry, CheckpointNode, REQUIRED_CHANNELS, ReducerKind};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("missing required channel: {0}")]
    MissingRequired(String),
    #[error("channel '{name}' has reducer {reducer:?} but value is not an array")]
    NotAnArray { name: String, reducer: ReducerKind },
    #[error("channel '{0}' has Sum reducer but value is not a number")]
    NotANumber(String),
}

pub fn merge_channels(ancestry: &[CheckpointNode]) -> BTreeMap<String, serde_json::Value> {
    let mut merged: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    for node in ancestry {
        for (key, entry) in &node.channels {
            let existing = merged.remove(key);
            let reduced = apply_reducer(&entry.reducer, existing, &entry.value);
            merged.insert(key.clone(), reduced);
        }
    }
    merged
}

pub fn apply_reducer(
    reducer: &ReducerKind,
    existing: Option<serde_json::Value>,
    new: &serde_json::Value,
) -> serde_json::Value {
    match reducer {
        ReducerKind::Overwrite => new.clone(),
        ReducerKind::Append => {
            let mut arr = match existing {
                Some(serde_json::Value::Array(a)) => a,
                _ => Vec::new(),
            };
            if let serde_json::Value::Array(new_arr) = new {
                arr.extend(new_arr.iter().cloned());
            }
            serde_json::Value::Array(arr)
        }
        ReducerKind::SetUnion => {
            let mut set: Vec<serde_json::Value> = match existing {
                Some(serde_json::Value::Array(a)) => a,
                _ => Vec::new(),
            };
            if let serde_json::Value::Array(new_arr) = new {
                for item in new_arr {
                    if !set.contains(item) {
                        set.push(item.clone());
                    }
                }
            }
            serde_json::Value::Array(set)
        }
        ReducerKind::Sum => {
            let existing_num = existing.and_then(|v| v.as_f64()).unwrap_or(0.0);
            let new_num = new.as_f64().unwrap_or(0.0);
            serde_json::Value::from(existing_num + new_num)
        }
    }
}

pub fn validate_channels(channels: &BTreeMap<String, ChannelEntry>) -> Result<(), ChannelError> {
    for &name in REQUIRED_CHANNELS {
        if !channels.contains_key(name) {
            return Err(ChannelError::MissingRequired(name.to_string()));
        }
    }

    for (name, entry) in channels {
        match entry.reducer {
            ReducerKind::Append | ReducerKind::SetUnion => {
                if !entry.value.is_array() {
                    return Err(ChannelError::NotAnArray {
                        name: name.clone(),
                        reducer: entry.reducer,
                    });
                }
            }
            ReducerKind::Sum => {
                if !entry.value.is_number() {
                    return Err(ChannelError::NotANumber(name.clone()));
                }
            }
            ReducerKind::Overwrite => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{ChannelEntry, CheckpointNode, ReducerKind};
    use crate::id::{AgentId, CheckpointId};
    use chrono::Utc;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn make_node(channels: BTreeMap<String, ChannelEntry>) -> CheckpointNode {
        CheckpointNode {
            id: CheckpointId::new(),
            agent_id: AgentId::new(),
            parent_id: None,
            branch_label: None,
            channels,
            l2_namespace: "general".into(),
            interrupt: None,
            created_at: Utc::now(),
        }
    }

    fn entry(reducer: ReducerKind, value: serde_json::Value) -> ChannelEntry {
        ChannelEntry { reducer, value }
    }

    #[test]
    fn overwrite_replaces_value() {
        let n1 = make_node(BTreeMap::from([(
            "progress_summary".into(),
            entry(ReducerKind::Overwrite, json!("old")),
        )]));
        let n2 = make_node(BTreeMap::from([(
            "progress_summary".into(),
            entry(ReducerKind::Overwrite, json!("new")),
        )]));
        let merged = merge_channels(&[n1, n2]);
        assert_eq!(merged["progress_summary"], json!("new"));
    }

    #[test]
    fn append_concatenates_arrays() {
        let n1 = make_node(BTreeMap::from([(
            "decisions".into(),
            entry(ReducerKind::Append, json!(["a", "b"])),
        )]));
        let n2 = make_node(BTreeMap::from([(
            "decisions".into(),
            entry(ReducerKind::Append, json!(["c"])),
        )]));
        let merged = merge_channels(&[n1, n2]);
        assert_eq!(merged["decisions"], json!(["a", "b", "c"]));
    }

    #[test]
    fn set_union_deduplicates() {
        let n1 = make_node(BTreeMap::from([(
            "files".into(),
            entry(ReducerKind::SetUnion, json!(["a", "b"])),
        )]));
        let n2 = make_node(BTreeMap::from([(
            "files".into(),
            entry(ReducerKind::SetUnion, json!(["b", "c"])),
        )]));
        let merged = merge_channels(&[n1, n2]);
        let arr = merged["files"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert!(arr.contains(&json!("a")));
        assert!(arr.contains(&json!("b")));
        assert!(arr.contains(&json!("c")));
    }

    #[test]
    fn sum_adds_numbers() {
        let n1 = make_node(BTreeMap::from([(
            "count".into(),
            entry(ReducerKind::Sum, json!(5)),
        )]));
        let n2 = make_node(BTreeMap::from([(
            "count".into(),
            entry(ReducerKind::Sum, json!(3)),
        )]));
        let merged = merge_channels(&[n1, n2]);
        assert_eq!(merged["count"], json!(8.0));
    }

    #[test]
    fn append_from_none() {
        let n1 = make_node(BTreeMap::from([(
            "blockers".into(),
            entry(ReducerKind::Append, json!(["x"])),
        )]));
        let merged = merge_channels(&[n1]);
        assert_eq!(merged["blockers"], json!(["x"]));
    }

    #[test]
    fn validate_channels_ok() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "objectives".into(),
            entry(ReducerKind::Overwrite, json!([])),
        );
        channels.insert(
            "progress_summary".into(),
            entry(ReducerKind::Overwrite, json!("")),
        );
        channels.insert("decisions".into(), entry(ReducerKind::Append, json!([])));
        channels.insert("blockers".into(), entry(ReducerKind::Append, json!([])));
        assert!(validate_channels(&channels).is_ok());
    }

    #[test]
    fn validate_channels_missing_required() {
        let channels = BTreeMap::new();
        assert!(validate_channels(&channels).is_err());
    }

    #[test]
    fn validate_channels_append_must_be_array() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "objectives".into(),
            entry(ReducerKind::Overwrite, json!([])),
        );
        channels.insert(
            "progress_summary".into(),
            entry(ReducerKind::Overwrite, json!("")),
        );
        channels.insert(
            "decisions".into(),
            entry(ReducerKind::Append, json!("not an array")),
        );
        channels.insert("blockers".into(), entry(ReducerKind::Append, json!([])));
        assert!(validate_channels(&channels).is_err());
    }

    #[test]
    fn validate_channels_sum_must_be_number() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "objectives".into(),
            entry(ReducerKind::Overwrite, json!([])),
        );
        channels.insert(
            "progress_summary".into(),
            entry(ReducerKind::Overwrite, json!("")),
        );
        channels.insert("decisions".into(), entry(ReducerKind::Append, json!([])));
        channels.insert("blockers".into(), entry(ReducerKind::Append, json!([])));
        channels.insert(
            "count".into(),
            entry(ReducerKind::Sum, json!("not a number")),
        );
        assert!(validate_channels(&channels).is_err());
    }
}
