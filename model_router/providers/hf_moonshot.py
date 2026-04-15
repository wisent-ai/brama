"""
Model Router - HuggingFace and Moonshot providers.
"""

import os
import time
import logging
from typing import Optional

from model_router.types import ModelProvider, ModelRequest, ModelResponse

logger = logging.getLogger(__name__)


class HuggingFaceProvider(ModelProvider):
    """
    HuggingFace Inference API provider.

    Uses the HF Router endpoint for OpenAI-compatible inference.
    Set HF_TOKEN environment variable or have ~/.cache/huggingface/token.

    Supported models:
    - Qwen/Qwen2.5-72B-Instruct
    - meta-llama/Llama-3.3-70B-Instruct
    - (abliterated models NOT supported on serverless)
    """

    # NOTE: Only these models are available on HF serverless
    # Abliterated models are NOT supported on serverless
    MODELS = {
        "qwen-72b": "Qwen/Qwen2.5-72B-Instruct",
        "llama-70b": "meta-llama/Llama-3.3-70B-Instruct",
    }

    # HF serverless is pay-per-token
    PRICING = {
        "qwen-72b": (0.0007, 0.0007),
        "llama-70b": (0.0007, 0.0007),
    }

    def __init__(self):
        self.api_key = os.getenv("HF_TOKEN")
        if not self.api_key:
            hf_token_path = os.path.expanduser("~/.cache/huggingface/token")
            if os.path.exists(hf_token_path):
                with open(hf_token_path) as f:
                    self.api_key = f.read().strip()
        self._client = None
        self._available = None

    @property
    def client(self):
        if self._client is None and self.api_key:
            try:
                from openai import AsyncOpenAI
                self._client = AsyncOpenAI(
                    api_key=self.api_key,
                    base_url="https://router.huggingface.co/v1",
                )
            except ImportError:
                logger.error("openai package not installed")
        return self._client

    async def is_available(self) -> bool:
        if self._available is not None:
            return self._available
        self._available = self.client is not None and self.api_key is not None
        return self._available

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
                error="HuggingFace token not configured (set HF_TOKEN or login with huggingface-cli)",
            )

        try:
            model_id = self.MODELS.get(request.model, request.model)

            messages = request.messages.copy()
            if request.system:
                messages.insert(0, {"role": "system", "content": request.system})

            response = await self.client.chat.completions.create(
                model=model_id,
                messages=messages,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
            )

            latency = (time.time() - start_time) * 1000
            input_tokens = response.usage.prompt_tokens if response.usage else 0
            output_tokens = response.usage.completion_tokens if response.usage else 0

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
            logger.error(f"HuggingFace API error: {e}")
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
        input_price, output_price = self.PRICING.get("qwen-72b", (0.0007, 0.0007))
        return (input_tokens / 1000 * input_price) + (output_tokens / 1000 * output_price)


class MoonshotProvider(ModelProvider):
    """
    Moonshot AI provider (Kimi models).

    Set MOONSHOT_API_KEY environment variable.

    Supported models:
    - kimi-k2-0711-preview (Kimi 2.5 with thinking)
    """

    MODELS = {
        "kimi-2.5": "kimi-k2-0711-preview",
    }

    PRICING = {
        "kimi-2.5": (0.002, 0.002),
    }

    def __init__(self):
        self.api_key = os.getenv("MOONSHOT_API_KEY")
        self._client = None
        self._available = None

    @property
    def client(self):
        if self._client is None and self.api_key:
            try:
                from openai import AsyncOpenAI
                self._client = AsyncOpenAI(
                    api_key=self.api_key,
                    base_url="https://api.moonshot.cn/v1",
                )
            except ImportError:
                logger.error("openai package not installed")
        return self._client

    async def is_available(self) -> bool:
        if self._available is not None:
            return self._available
        self._available = self.client is not None and self.api_key is not None
        return self._available

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
                error="Moonshot API key not configured (set MOONSHOT_API_KEY)",
            )

        try:
            model_id = self.MODELS.get(request.model, request.model)

            messages = request.messages.copy()
            if request.system:
                messages.insert(0, {"role": "system", "content": request.system})

            response = await self.client.chat.completions.create(
                model=model_id,
                messages=messages,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
            )

            latency = (time.time() - start_time) * 1000
            input_tokens = response.usage.prompt_tokens if response.usage else 0
            output_tokens = response.usage.completion_tokens if response.usage else 0

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
            logger.error(f"Moonshot API error: {e}")
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
        input_price, output_price = self.PRICING.get("kimi-2.5", (0.002, 0.002))
        return (input_tokens / 1000 * input_price) + (output_tokens / 1000 * output_price)
