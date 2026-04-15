use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tracing::{error, info, warn};

use crate::provider::ModelProvider;
use crate::types::{Message, ModelRequest, ModelResponse};

pub struct RouterStats {
    pub total_requests: AtomicU64,
    pub total_input_tokens: AtomicU64,
    pub total_output_tokens: AtomicU64,
}

impl RouterStats {
    fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            total_input_tokens: AtomicU64::new(0),
            total_output_tokens: AtomicU64::new(0),
        }
    }

    fn record(&self, resp: &ModelResponse) {
        self.total_requests
            .fetch_add(1, Ordering::Relaxed);
        self.total_input_tokens.fetch_add(
            resp.input_tokens as u64,
            Ordering::Relaxed,
        );
        self.total_output_tokens.fetch_add(
            resp.output_tokens as u64,
            Ordering::Relaxed,
        );
    }
}

pub struct ModelRouter {
    providers: HashMap<String, Arc<dyn ModelProvider>>,
    model_to_provider: HashMap<String, String>,
    retry_chains: HashMap<String, Vec<String>>,
    pub stats: RouterStats,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            model_to_provider: HashMap::new(),
            retry_chains: HashMap::new(),
            stats: RouterStats::new(),
        }
    }

    pub fn register_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
    ) {
        let name = provider.name().to_string();
        for model in provider.models() {
            self.model_to_provider
                .insert(model, name.clone());
        }
        self.providers.insert(name, provider);
    }

    pub fn set_retry_chain(
        &mut self,
        model: &str,
        chain: Vec<String>,
    ) {
        self.retry_chains
            .insert(model.to_string(), chain);
    }

    pub async fn complete(
        &self,
        messages: Vec<Message>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
        system: Option<String>,
    ) -> ModelResponse {
        let request = ModelRequest {
            messages,
            model: model.to_string(),
            max_tokens,
            temperature,
            system,
        };

        // Try the primary provider for this model
        if let Some(pname) =
            self.model_to_provider.get(model)
        {
            if let Some(provider) = self.providers.get(pname)
            {
                info!(
                    "Routing {} to provider {}",
                    model, pname
                );
                let resp =
                    provider.complete(&request).await;
                if resp.success {
                    self.stats.record(&resp);
                    return resp;
                }
                warn!(
                    "Primary {} failed for {}: {:?}",
                    pname, model, resp.error
                );
            }
        }

        // Walk the retry chain to find an alternative
        if let Some(chain) = self.retry_chains.get(model) {
            for alt_model in chain {
                let pname = match self
                    .model_to_provider
                    .get(alt_model)
                {
                    Some(n) => n,
                    None => continue,
                };
                let provider =
                    match self.providers.get(pname) {
                        Some(p) => p,
                        None => continue,
                    };

                info!(
                    "Retry with {} via {}",
                    alt_model, pname
                );

                let alt_req = ModelRequest {
                    messages: request.messages.clone(),
                    model: alt_model.clone(),
                    max_tokens: request.max_tokens,
                    temperature: request.temperature,
                    system: request.system.clone(),
                };

                let resp =
                    provider.complete(&alt_req).await;
                if resp.success {
                    self.stats.record(&resp);
                    return resp;
                }
                warn!(
                    "Retry {} failed: {:?}",
                    alt_model, resp.error
                );
            }
        }

        error!("All providers failed for {}", model);
        ModelResponse::failure(
            model,
            format!("all providers failed for {model}"),
        )
    }

    pub async fn get_available_providers(
        &self,
    ) -> HashMap<String, bool> {
        let mut result = HashMap::new();
        for (name, provider) in &self.providers {
            let avail = provider.is_available().await;
            result.insert(name.clone(), avail);
        }
        result
    }

    pub fn get_cheapest_model(
        &self,
        _min_quality: f64,
    ) -> Option<String> {
        let ranked = [
            "qwen3-4b",
            "qwen3-8b",
            "deepseek-r1-qwen3-8b",
            "gpt-4o-mini",
            "kimi-2.5",
            "qwen-72b",
            "llama-70b",
            "gpt-4o",
            "claude-haiku-3.5",
            "claude-sonnet-4",
            "claude-opus-4",
            "claude-opus-4-5",
        ];

        for m in &ranked {
            if self.model_to_provider.contains_key(*m) {
                return Some(m.to_string());
            }
        }
        None
    }

    pub fn all_models(&self) -> Vec<String> {
        self.model_to_provider.keys().cloned().collect()
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn build_default_router() -> ModelRouter {
    use crate::providers::*;

    let mut router = ModelRouter::new();

    router.register_provider(Arc::new(
        AnthropicProvider::new(),
    ));
    router.register_provider(Arc::new(
        OpenAIProvider::new(),
    ));
    router.register_provider(Arc::new(
        HuggingFaceProvider::new(),
    ));
    router.register_provider(Arc::new(
        MoonshotProvider::new(),
    ));
    router.register_provider(Arc::new(
        LocalProvider::new(),
    ));
    router.register_provider(Arc::new(
        VertexProvider::new(),
    ));

    // Retry chains: if primary fails, try alternatives
    router.set_retry_chain(
        "claude-sonnet-4",
        vec!["gpt-4o".into(), "llama-70b".into()],
    );
    router.set_retry_chain(
        "gpt-4o",
        vec![
            "claude-sonnet-4".into(),
            "llama-70b".into(),
        ],
    );
    router.set_retry_chain(
        "gpt-4o-mini",
        vec![
            "claude-haiku-3.5".into(),
            "kimi-2.5".into(),
        ],
    );

    router
}
