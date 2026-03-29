use tokio::sync::oneshot;

/// A pending human-in-the-loop request from an agent.
#[derive(Debug)]
pub struct HitlRequest {
    pub question: String,
    pub options: Vec<String>,
    pub response_tx: oneshot::Sender<String>,
}
