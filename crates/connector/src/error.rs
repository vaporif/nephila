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

    #[error("process error (exit code {exit_code:?}): {stderr}")]
    Process {
        exit_code: Option<i32>,
        stderr: String,
    },

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
    fn other_error_converts_to_nephila_error() {
        let inner = std::io::Error::new(std::io::ErrorKind::Other, "boom");
        let err = ConnectorError::Other(Box::new(inner));
        let me: NephilaError = err.into();
        assert!(matches!(me, NephilaError::Connector(_)));
    }
}
