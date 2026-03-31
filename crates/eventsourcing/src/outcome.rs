use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Success,
    Failure { reason: String },
    Partial { detail: String },
}
