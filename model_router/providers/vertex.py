"""
Model Router - Vertex AI provider (Claude + Gemini via GCP).
"""

import os
import time
import asyncio
import logging
from typing import Optional

from model_router.types import ModelProvider, ModelRequest, ModelResponse

logger = logging.getLogger(__name__)


class VertexAIProvider(ModelProvider):
    """
    Vertex AI provider - supports Claude and Gemini models via GCP.

    Uses Application Default Credentials (ADC):
    - Run `gcloud auth application-default login` locally
    - Or use a service account in production

    Environment variables:
    - VERTEX_PROJECT: GCP project ID
    - VERTEX_LOCATION: GCP region (default: us-central1)
    """

    # Claude models via Vertex AI (use Vertex-specific model IDs)
    CLAUDE_MODELS = {
        "claude-opus-4.5": "claude-opus-4-5@20250514",
        "claude-opus-4": "claude-opus-4@20250514",
        "claude-sonnet-4": "claude-sonnet-4@20250514",
        "claude-sonnet-3.5": "claude-3-5-sonnet-v2@20241022",
        "claude-haiku-3.5": "claude-3-5-haiku@20241022",
        "claude-opus-3": "claude-3-opus@20240229",
    }

    # Gemini models (native to Vertex)
    GEMINI_MODELS = {
        "gemini-2.0-flash": "gemini-2.0-flash-001",
        "gemini-1.5-pro": "gemini-1.5-pro-002",
        "gemini-1.5-flash": "gemini-1.5-flash-002",
    }

    PRICING = {
        # Claude pricing on Vertex (same as direct)
        "claude-opus-4.5": (0.015, 0.075),
        "claude-opus-4": (0.015, 0.075),
        "claude-sonnet-4": (0.003, 0.015),
        "claude-sonnet-3.5": (0.003, 0.015),
        "claude-haiku-3.5": (0.001, 0.005),
        "claude-opus-3": (0.015, 0.075),
        # Gemini pricing
        "gemini-2.0-flash": (0.00035, 0.0015),
        "gemini-1.5-pro": (0.00125, 0.005),
        "gemini-1.5-flash": (0.000075, 0.0003),
    }

    def __init__(self):
        self.project = os.getenv("VERTEX_PROJECT") or os.getenv("GCP_PROJECT") or os.getenv("GOOGLE_CLOUD_PROJECT")
        self.location = os.getenv("VERTEX_LOCATION", "us-central1")
        self._claude_client = None
        self._gemini_client = None
        self._available = None

    @property
    def claude_client(self):
        """Anthropic client for Claude models via Vertex AI."""
        if self._claude_client is None and self.project:
            try:
                from anthropic import AnthropicVertex
                self._claude_client = AnthropicVertex(
                    project_id=self.project,
                    region=self.location,
                )
            except ImportError:
                logger.error("anthropic[vertex] package not installed")
            except Exception as e:
                logger.error(f"Failed to initialize Vertex AI Claude client: {e}")
        return self._claude_client

    @property
    def gemini_client(self):
        """Vertex AI client for Gemini models."""
        if self._gemini_client is None and self.project:
            try:
                import vertexai
                from vertexai.generative_models import GenerativeModel
                vertexai.init(project=self.project, location=self.location)
                self._gemini_client = True  # Mark as initialized
            except ImportError:
                logger.error("google-cloud-aiplatform package not installed")
            except Exception as e:
                logger.error(f"Failed to initialize Vertex AI Gemini client: {e}")
        return self._gemini_client

    async def is_available(self) -> bool:
        if self._available is not None:
            return self._available

        # Check if we have project configured and at least one client works
        if not self.project:
            self._available = False
            return False

        self._available = self.claude_client is not None or self.gemini_client is not None
        return self._available

    def _is_claude_model(self, model: str) -> bool:
        return model in self.CLAUDE_MODELS or model.startswith("claude")

    def _is_gemini_model(self, model: str) -> bool:
        return model in self.GEMINI_MODELS or model.startswith("gemini")

    async def complete(self, request: ModelRequest) -> ModelResponse:
        start_time = time.time()

        if not await self.is_available():
            return ModelResponse(
                content="",
                model=request.model,
                input_tokens=0,
                output_tokens=0,
                latency_ms=0,
                cost=0,
                success=False,
                error="Vertex AI not configured (set VERTEX_PROJECT or GCP_PROJECT)",
            )

        try:
            if self._is_claude_model(request.model):
                return await self._complete_claude(request, start_time)
            elif self._is_gemini_model(request.model):
                return await self._complete_gemini(request, start_time)
            else:
                return ModelResponse(
                    content="",
                    model=request.model,
                    input_tokens=0,
                    output_tokens=0,
                    latency_ms=0,
                    cost=0,
                    success=False,
                    error=f"Unknown model for Vertex AI: {request.model}",
                )

        except Exception as e:
            logger.error(f"Vertex AI error: {e}")
            return ModelResponse(
                content="",
                model=request.model,
                input_tokens=0,
                output_tokens=0,
                latency_ms=(time.time() - start_time) * 1000,
                cost=0,
                success=False,
                error=str(e),
            )

    async def _complete_claude(self, request: ModelRequest, start_time: float) -> ModelResponse:
        """Complete using Claude via Vertex AI."""
        if not self.claude_client:
            return ModelResponse(
                content="",
                model=request.model,
                input_tokens=0,
                output_tokens=0,
                latency_ms=0,
                cost=0,
                success=False,
                error="Claude client not available on Vertex AI",
            )

        model_id = self.CLAUDE_MODELS.get(request.model, request.model)

        # Run sync client in executor (AnthropicVertex doesn't have async)
        loop = asyncio.get_event_loop()
        response = await loop.run_in_executor(
            None,
            lambda: self.claude_client.messages.create(
                model=model_id,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
                system=request.system or "You are a helpful assistant.",
                messages=request.messages,
            )
        )

        latency = (time.time() - start_time) * 1000
        input_tokens = response.usage.input_tokens
        output_tokens = response.usage.output_tokens

        return ModelResponse(
            content=response.content[0].text,
            model=request.model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            latency_ms=latency,
            cost=self.estimate_cost(input_tokens, output_tokens),
            success=True,
        )

    async def _complete_gemini(self, request: ModelRequest, start_time: float) -> ModelResponse:
        """Complete using Gemini via Vertex AI."""
        if not self.gemini_client:
            return ModelResponse(
                content="",
                model=request.model,
                input_tokens=0,
                output_tokens=0,
                latency_ms=0,
                cost=0,
                success=False,
                error="Gemini client not available on Vertex AI",
            )

        from vertexai.generative_models import GenerativeModel, GenerationConfig

        model_id = self.GEMINI_MODELS.get(request.model, request.model)
        model = GenerativeModel(model_id)

        # Build prompt with system message
        messages = request.messages.copy()
        if request.system:
            messages.insert(0, {"role": "user", "content": f"System: {request.system}"})

        # Convert messages to Gemini format
        gemini_messages = []
        for msg in messages:
            role = "user" if msg["role"] == "user" else "model"
            gemini_messages.append({"role": role, "parts": [{"text": msg["content"]}]})

        config = GenerationConfig(
            max_output_tokens=request.max_tokens,
            temperature=request.temperature,
        )

        # Run in executor (Gemini SDK is sync)
        loop = asyncio.get_event_loop()
        response = await loop.run_in_executor(
            None,
            lambda: model.generate_content(gemini_messages, generation_config=config)
        )

        latency = (time.time() - start_time) * 1000

        # Extract token counts
        input_tokens = response.usage_metadata.prompt_token_count if hasattr(response, 'usage_metadata') else 0
        output_tokens = response.usage_metadata.candidates_token_count if hasattr(response, 'usage_metadata') else 0

        return ModelResponse(
            content=response.text,
            model=request.model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            latency_ms=latency,
            cost=self.estimate_cost(input_tokens, output_tokens),
            success=True,
        )

    def estimate_cost(self, input_tokens: int, output_tokens: int) -> float:
        # Use gemini-1.5-flash as default pricing
        input_price, output_price = self.PRICING.get("gemini-1.5-flash", (0.000075, 0.0003))
        return (input_tokens / 1000 * input_price) + (output_tokens / 1000 * output_price)
