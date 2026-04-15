"""
Model Router - Cloud API providers (Anthropic, OpenAI).
"""

import os
import time
import logging
from typing import Optional

from model_router.types import ModelProvider, ModelRequest, ModelResponse

logger = logging.getLogger(__name__)


class AnthropicProvider(ModelProvider):
    """Anthropic Claude models via direct API. Real API calls only."""

    MODELS = {
        "claude-opus-4.5": "claude-opus-4-5-20250514",
        "claude-opus-4": "claude-opus-4-20250514",
        "claude-sonnet-4": "claude-sonnet-4-20250514",
        "claude-haiku-3.5": "claude-3-5-haiku-20241022",
    }

    PRICING = {
        "claude-opus-4.5": (0.015, 0.075),
        "claude-opus-4": (0.015, 0.075),
        "claude-sonnet-4": (0.003, 0.015),
        "claude-haiku-3.5": (0.001, 0.005),
    }

    def __init__(self):
        self.api_key = os.getenv("ANTHROPIC_API_KEY")
        self._client = None

    @property
    def client(self):
        if self._client is None and self.api_key:
            try:
                import anthropic
                self._client = anthropic.AsyncAnthropic(api_key=self.api_key)
            except ImportError:
                logger.error("anthropic package not installed")
        return self._client

    async def is_available(self) -> bool:
        return self.client is not None and self.api_key is not None

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
                error="Anthropic API key not configured or package not installed",
            )

        try:
            model_id = self.MODELS.get(request.model, request.model)

            response = await self.client.messages.create(
                model=model_id,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
                system=request.system or "You are a helpful assistant.",
                messages=request.messages,
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

        except Exception as e:
            logger.error(f"Anthropic API error: {e}")
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

    def estimate_cost(self, input_tokens: int, output_tokens: int) -> float:
        input_price, output_price = self.PRICING.get("claude-haiku-3.5", (0.001, 0.005))
        return (input_tokens / 1000 * input_price) + (output_tokens / 1000 * output_price)


class OpenAIProvider(ModelProvider):
    """OpenAI GPT models. Real API calls only."""

    MODELS = {
        "gpt-4o": "gpt-4o",
        "gpt-4o-mini": "gpt-4o-mini",
    }

    PRICING = {
        "gpt-4o": (0.005, 0.015),
        "gpt-4o-mini": (0.00015, 0.0006),
    }

    def __init__(self):
        self.api_key = os.getenv("OPENAI_API_KEY")
        self._client = None

    @property
    def client(self):
        if self._client is None and self.api_key:
            try:
                import openai
                self._client = openai.AsyncOpenAI(api_key=self.api_key)
            except ImportError:
                logger.error("openai package not installed")
        return self._client

    async def is_available(self) -> bool:
        return self.client is not None and self.api_key is not None

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
                error="OpenAI API key not configured or package not installed",
            )

        try:
            messages = request.messages.copy()
            if request.system:
                messages.insert(0, {"role": "system", "content": request.system})

            response = await self.client.chat.completions.create(
                model=self.MODELS.get(request.model, request.model),
                messages=messages,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
            )

            latency = (time.time() - start_time) * 1000
            input_tokens = response.usage.prompt_tokens
            output_tokens = response.usage.completion_tokens

            return ModelResponse(
                content=response.choices[0].message.content,
                model=request.model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                latency_ms=latency,
                cost=self.estimate_cost(input_tokens, output_tokens),
                success=True,
            )

        except Exception as e:
            logger.error(f"OpenAI API error: {e}")
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

    def estimate_cost(self, input_tokens: int, output_tokens: int) -> float:
        input_price, output_price = self.PRICING.get("gpt-4o-mini", (0.00015, 0.0006))
        return (input_tokens / 1000 * input_price) + (output_tokens / 1000 * output_price)
