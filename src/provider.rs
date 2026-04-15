use async_trait::async_trait;

use crate::types::{ModelRequest, ModelResponse};

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(&self, request: &ModelRequest) -> ModelResponse;
    fn estimate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64;
    async fn is_available(&self) -> bool;
    fn name(&self) -> &str;
    fn models(&self) -> Vec<String>;
}
