use async_trait::async_trait;

use crate::provider::ModelProvider;
use crate::types::{ModelRequest, ModelResponse};

// TODO: Vertex AI requires Google Cloud OAuth2
// authentication which involves service account credentials,
// token exchange, and token refresh. This needs a dedicated
// implementation using google-cloud-auth or similar crate.

pub struct VertexProvider;

impl VertexProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VertexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for VertexProvider {
    async fn complete(
        &self,
        request: &ModelRequest,
    ) -> ModelResponse {
        ModelResponse::failure(
            &request.model,
            "Vertex AI provider not yet implemented".into(),
        )
    }

    fn estimate_cost(
        &self,
        _input_tokens: u32,
        _output_tokens: u32,
    ) -> f64 {
        0.0
    }

    async fn is_available(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "vertex"
    }

    fn models(&self) -> Vec<String> {
        Vec::new()
    }
}
