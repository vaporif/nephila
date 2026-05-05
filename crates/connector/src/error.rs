use std::time::Duration;

use nephila_core::NephilaError;

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("authentication error: {0}")]
    Auth(String),

    #[error("rate limited")]
    RateLimit { retry_after: Option<Duration> },

    #[error("request refused: {0}")]
    Refused(String),

    #[error("context overflow: used {used} of {limit}")]
    ContextOverflow { used: u64, limit: u64 },

    #[error("transport error: {0}")]
    Transport(String),

    /// Subprocess exited non-zero with captured stderr.
    #[error("process error (exit code {exit_code:?}): {stderr}")]
    Process {
        exit_code: Option<i32>,
        stderr: String,
    },

    /// Failure spawning a child process or performing other I/O syscalls
    /// associated with the subprocess lifecycle.
    #[error("spawn error: {0}")]
    Spawn(#[from] std::io::Error),

    /// Event-store append / load failure surfaced from the connector's
    /// reader/writer tasks.
    #[error("persist error: {0}")]
    Persist(#[from] nephila_eventsourcing::store::EventStoreError),

    /// JSON encoding/decoding failure when serialising envelopes or
    /// claude protocol payloads.
    #[error("serialize error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("{0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl From<ConnectorError> for NephilaError {
    fn from(err: ConnectorError) -> Self {
        Self::Connector(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_converts_to_nephila_error() {
        let err = ConnectorError::Auth("bad key".into());
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
        assert!(me.to_string().contains("authentication error: bad key"));
    }

    #[test]
    fn rate_limit_converts_to_nephila_error() {
        let err = ConnectorError::RateLimit {
            retry_after: Some(Duration::from_secs(30)),
        };
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
    }

    #[test]
    fn process_error_converts_to_nephila_error() {
        let err = ConnectorError::Process {
            exit_code: Some(1),
            stderr: "segfault".into(),
        };
        let me: NephilaError = err.into();
        assert!(me.to_string().contains("segfault"));
    }

    #[test]
    fn spawn_error_converts_to_nephila_error() {
        let io_err = std::io::Error::other("fork failed");
        let err = ConnectorError::Spawn(io_err);
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
        assert!(me.to_string().contains("spawn error"));
    }

    #[test]
    fn persist_error_converts_to_nephila_error() {
        let store_err = nephila_eventsourcing::store::EventStoreError::storage_msg("db down");
        let err = ConnectorError::Persist(store_err);
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
        assert!(me.to_string().contains("persist error"));
    }

    #[test]
    fn serialize_error_converts_to_nephila_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json")
            .expect_err("invalid json must fail to parse");
        let err = ConnectorError::Serialize(json_err);
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
        assert!(me.to_string().contains("serialize error"));
    }

    #[test]
    fn other_error_converts_to_nephila_error() {
        let inner = std::io::Error::other("boom");
        let err = ConnectorError::Other(Box::new(inner));
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
    }
}
