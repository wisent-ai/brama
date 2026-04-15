"""
Model Router - Routes requests to appropriate AI models.

Supports: Anthropic, OpenAI, HuggingFace, Moonshot, Vertex AI, Local vLLM/MLX, Abliterated
"""

from .types import ComputeResources, ModelRequest, ModelResponse, ModelProvider
from .detection import detect_compute_resources, select_model_for_resources
from .providers import (
    AnthropicProvider, OpenAIProvider, HuggingFaceProvider, MoonshotProvider,
    VertexAIProvider, LocalLlamaProvider, AbliteratedProvider,
    LocalInferenceManager, get_local_inference_manager,
)
from .router import ModelRouter, get_model_router

__all__ = [
    "ComputeResources", "ModelRequest", "ModelResponse", "ModelProvider",
    "detect_compute_resources", "select_model_for_resources",
    "AnthropicProvider", "OpenAIProvider", "HuggingFaceProvider", "MoonshotProvider",
    "VertexAIProvider", "LocalLlamaProvider", "AbliteratedProvider",
    "LocalInferenceManager", "get_local_inference_manager",
    "ModelRouter", "get_model_router",
]
