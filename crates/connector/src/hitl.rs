use crate::error::ConnectorError;

#[derive(Debug, Clone)]
pub struct HitlRequest {
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HitlResponse {
    pub answer: String,
}

pub trait HitlHandler: Send + Sync {
    fn on_input_requested(
        &self,
        request: HitlRequest,
    ) -> impl std::future::Future<Output = Result<HitlResponse, ConnectorError>> + Send;
}
