"""
Model Router - Main ModelRouter class and routing logic.
"""

import logging
from typing import Dict, List, Optional

from .types import ModelRequest, ModelResponse
from .providers import (
    get_local_inference_manager,
    VertexAIProvider, AnthropicProvider, OpenAIProvider,
    HuggingFaceProvider, MoonshotProvider,
    LocalLlamaProvider, AbliteratedProvider,
)

logger = logging.getLogger(__name__)


class ModelRouter:
    """
    Routes requests to appropriate model providers.

    NO SIMULATIONS. Real API calls with proper alternate chain.
    Auto-starts local inference when all providers are unavailable.

    Provider priority:
    1. Vertex AI (if GCP configured) - supports Claude + Gemini
    2. Direct Anthropic/OpenAI (if API keys set)
    3. Local inference (auto-started if needed)
    """

    def __init__(self, auto_start_local: bool = True):
        self.providers = {
            "vertex": VertexAIProvider(),
            "anthropic": AnthropicProvider(),
            "openai": OpenAIProvider(),
            "huggingface": HuggingFaceProvider(),
            "moonshot": MoonshotProvider(),
            "local_llama": LocalLlamaProvider(),
            "abliterated": AbliteratedProvider(),
        }

        # Model to provider mapping
        # Vertex AI is preferred for Claude models (uses GCP billing)
        self.model_providers = {
            # Claude models - prefer Vertex AI, alternate to direct
            "claude-opus-4.5": "vertex",
            "claude-opus-4": "vertex",
            "claude-sonnet-4": "vertex",
            "claude-haiku-3.5": "vertex",
            # Gemini models - Vertex AI only
            "gemini-2.0-flash": "vertex",
            "gemini-1.5-pro": "vertex",
            "gemini-1.5-flash": "vertex",
            # OpenAI models
            "gpt-4o": "openai",
            "gpt-4o-mini": "openai",
            # HuggingFace models (only 72B+ available on serverless)
            "qwen-72b": "huggingface",
            "llama-70b": "huggingface",
            # Moonshot/Kimi models
            "kimi-2.5": "moonshot",
            # Abliterated models (require local vLLM server on port 8001)
            "abliterated-glm4": "abliterated",
            "abliterated-llama-70b": "abliterated",
            "abliterated-llama-8b": "abliterated",
            "abliterated-gpt-120b": "abliterated",
            # Local models
            "deepseek-r1-qwen3-8b": "local_llama",
            "qwen3-8b": "local_llama",
            "qwen3-4b": "local_llama",
        }

        self.alternates = {
            # Claude alternates - try Vertex first, then HF, then OpenAI
            "claude-opus-4.5": ["claude-opus-4", "claude-sonnet-4", "qwen-72b", "gemini-1.5-pro"],
            "claude-opus-4": ["claude-sonnet-4", "qwen-72b", "gemini-1.5-pro", "gpt-4o"],
            "claude-sonnet-4": ["claude-haiku-3.5", "qwen-72b", "gemini-2.0-flash", "gpt-4o-mini"],
            "claude-haiku-3.5": ["qwen-72b", "gemini-1.5-flash", "gpt-4o-mini", "llama-70b"],
            # Gemini alternates
            "gemini-2.0-flash": ["gemini-1.5-flash", "qwen-72b", "claude-haiku-3.5", "gpt-4o-mini"],
            "gemini-1.5-pro": ["gemini-2.0-flash", "qwen-72b", "claude-sonnet-4", "gpt-4o"],
            "gemini-1.5-flash": ["gemini-2.0-flash", "qwen-72b", "claude-haiku-3.5", "gpt-4o-mini"],
            # OpenAI alternates
            "gpt-4o": ["qwen-72b", "claude-sonnet-4", "gemini-1.5-pro", "gpt-4o-mini"],
            "gpt-4o-mini": ["qwen-72b", "claude-haiku-3.5", "gemini-1.5-flash", "llama-70b"],
            # HuggingFace alternates
            "qwen-72b": ["llama-70b", "claude-sonnet-4", "gemini-1.5-pro", "gpt-4o"],
            "llama-70b": ["qwen-72b", "claude-sonnet-4", "gemini-1.5-pro", "gpt-4o"],
            # Moonshot alternates
            "kimi-2.5": ["qwen-72b", "claude-sonnet-4", "gpt-4o", "gemini-1.5-pro"],
            # Abliterated alternates (try other abliterated, then uncensored-friendly)
            "abliterated-glm4": ["abliterated-llama-8b", "qwen-72b", "llama-70b"],
            "abliterated-llama-70b": ["abliterated-glm4", "qwen-72b", "llama-70b"],
            "abliterated-llama-8b": ["abliterated-glm4", "qwen-72b", "llama-70b"],
            "abliterated-gpt-120b": ["abliterated-llama-70b", "abliterated-glm4", "qwen-72b"],
            # Local alternates
            "deepseek-r1-qwen3-8b": ["qwen3-8b", "qwen3-4b", "qwen-72b", "gemini-1.5-flash"],
            "qwen3-8b": ["deepseek-r1-qwen3-8b", "qwen3-4b", "qwen-72b", "gemini-1.5-flash"],
            "qwen3-4b": ["deepseek-r1-qwen3-8b", "qwen3-8b", "qwen-72b", "gpt-4o-mini"],
        }

        self.total_requests = 0
        self.total_cost = 0.0
        self.total_tokens = 0
        self.auto_start_local = auto_start_local
        self.local_inference_manager = get_local_inference_manager()
        self._local_start_attempted = False

    async def get_available_providers(self) -> Dict[str, bool]:
        """Check which providers are currently available."""
        availability = {}
        for name, provider in self.providers.items():
            availability[name] = await provider.is_available()
        return availability

    async def _ensure_local_inference(self) -> bool:
        """Start local inference if not running and no other providers available."""
        if self._local_start_attempted:
            return self.local_inference_manager.is_running()
        self._local_start_attempted = True
        resources = self.local_inference_manager.get_resources()
        logger.info(f"Local compute: {resources.gpu_type or 'CPU'}, "
                   f"{resources.vram_gb:.1f}GB VRAM, {resources.ram_gb:.1f}GB RAM, "
                   f"{resources.cpu_cores} cores")
        started = await self.local_inference_manager.start()
        if started:
            self.providers["local_llama"] = LocalLlamaProvider(
                endpoint=f"http://localhost:{self.local_inference_manager.port}"
            )
            self.providers["local_llama"]._available = None
            logger.info("Local inference started and registered with router")
        return started

    async def complete(
        self,
        messages: List[Dict[str, str]],
        model: str = "claude-haiku-3.5",
        max_tokens: int = 4096,
        temperature: float = 0.7,
        system: Optional[str] = None,
        use_alternates: bool = True,
    ) -> ModelResponse:
        """Route request to appropriate provider with alternate chain."""
        request = ModelRequest(
            messages=messages, model=model,
            max_tokens=max_tokens, temperature=temperature, system=system,
        )
        response = await self._try_model(request)
        # Try alternates if failed
        if not response.success and use_alternates:
            for alt_model in self.alternates.get(model, []):
                logger.info(f"Primary model {model} failed, trying alternate {alt_model}")
                request.model = alt_model
                response = await self._try_model(request)
                if response.success:
                    logger.info(f"Alternate {alt_model} succeeded")
                    break
        # If all failed and auto_start_local enabled, try starting local inference
        if not response.success and self.auto_start_local:
            logger.info("All providers failed, attempting to start local inference...")
            if await self._ensure_local_inference():
                for local_model in ["deepseek-r1-qwen3-8b", "qwen3-8b", "qwen3-4b", "gemma3-12b"]:
                    request.model = local_model
                    response = await self._try_model(request)
                    if response.success:
                        logger.info(f"Local inference with {local_model} succeeded")
                        break
        self.total_requests += 1
        self.total_cost += response.cost
        self.total_tokens += response.input_tokens + response.output_tokens
        return response

    async def _try_model(self, request: ModelRequest) -> ModelResponse:
        """Try a single model."""
        provider_name = self.model_providers.get(request.model)
        if not provider_name:
            return ModelResponse(
                content="", model=request.model, input_tokens=0, output_tokens=0,
                latency_ms=0, cost=0, success=False,
                error=f"Unknown model: {request.model}",
            )
        provider = self.providers.get(provider_name)
        if not provider:
            return ModelResponse(
                content="", model=request.model, input_tokens=0, output_tokens=0,
                latency_ms=0, cost=0, success=False,
                error=f"Provider not available: {provider_name}",
            )
        return await provider.complete(request)

    def get_cheapest_model(self, min_quality: str = "low") -> str:
        """Get cheapest model meeting quality threshold."""
        quality_tiers = {
            "low": ["qwen3-4b", "gpt-4o-mini", "claude-haiku-3.5"],
            "medium": ["deepseek-r1-qwen3-8b", "qwen3-8b", "gpt-4o-mini"],
            "high": ["deepseek-r1-qwen3-8b", "claude-sonnet-4", "gpt-4o"],
        }
        models = quality_tiers.get(min_quality, quality_tiers["low"])
        return models[0] if models else "deepseek-r1-qwen3-8b"

    def get_stats(self) -> dict:
        """Get router statistics."""
        return {
            "total_requests": self.total_requests,
            "total_cost": self.total_cost,
            "total_tokens": self.total_tokens,
            "avg_cost_per_request": self.total_cost / self.total_requests if self.total_requests > 0 else 0,
        }


_router: Optional[ModelRouter] = None

def get_model_router() -> ModelRouter:
    """Get global model router instance."""
    global _router
    if _router is None:
        _router = ModelRouter()
    return _router
