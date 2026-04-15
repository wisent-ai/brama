"""
Model Router - Local/self-hosted providers (LocalLlama, Abliterated).
"""

import os
import time
import aiohttp
import logging
from typing import Optional

from model_router.types import ModelProvider, ModelRequest, ModelResponse

logger = logging.getLogger(__name__)


class LocalLlamaProvider(ModelProvider):
    """
    Local models via vLLM/MLX OpenAI-compatible endpoint.

    Supports vLLM (NVIDIA) or MLX (Apple Silicon) servers.

    2025/2026 Best Models:
    - DeepSeek-R1-Qwen3-8B: SOTA reasoning at 8B size
    - Qwen3-8B: Best instruction following
    - Gemma-3-12B: Best mid-size (#66 LMArena)
    - Qwen3-4B: Amazing for tiny size (74% MMLU-Pro)
    """

    MODELS = {
        "deepseek-r1-qwen3-8b": "mlx-community/deepseek-r1-0528-qwen3-8b-4bit",
        "qwen3-8b": "Qwen/Qwen3-8B",
        "qwen3-4b": "Qwen/Qwen3-4B",
    }

    GPU_COST_PER_HOUR = {
        "deepseek-r1-qwen3-8b": 0.15,
        "qwen3-8b": 0.15,
        "qwen3-4b": 0.08,
    }

    def __init__(self, endpoint: Optional[str] = None):
        self.endpoint = endpoint or os.getenv("LLAMA_ENDPOINT", "http://localhost:8000")
        self._available = None

    async def is_available(self) -> bool:
        """Check if vLLM endpoint is reachable."""
        if self._available is not None:
            return self._available

        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(
                    f"{self.endpoint}/health",

                ) as resp:
                    self._available = resp.status == 200
        except Exception:
            # Try /v1/models as secondary health check
            try:
                async with aiohttp.ClientSession() as session:
                    async with session.get(
                        f"{self.endpoint}/v1/models",

                    ) as resp:
                        self._available = resp.status == 200
            except Exception:
                self._available = False

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
                error=f"Local Llama endpoint not available at {self.endpoint}",
            )

        try:
            model_id = self.MODELS.get(request.model, request.model)

            messages = request.messages.copy()
            if request.system:
                messages.insert(0, {"role": "system", "content": request.system})

            payload = {
                "model": model_id,
                "messages": messages,
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
            }

            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f"{self.endpoint}/v1/chat/completions",
                    json=payload,

                ) as resp:
                    if resp.status != 200:
                        error_text = await resp.text()
                        return ModelResponse(
                            content="",
                            model=request.model,
                            input_tokens=0,
                            output_tokens=0,
                            latency_ms=(time.time() - start_time) * 1000,
                            cost=0,
                            success=False,
                            error=f"vLLM error {resp.status}: {error_text}",
                        )

                    data = await resp.json()

            latency = (time.time() - start_time) * 1000
            input_tokens = data.get("usage", {}).get("prompt_tokens", 0)
            output_tokens = data.get("usage", {}).get("completion_tokens", 0)
            content = data["choices"][0]["message"]["content"]

            return ModelResponse(
                content=content,
                model=request.model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                latency_ms=latency,
                cost=self.estimate_cost(input_tokens, output_tokens),
                success=True,
            )

        except Exception as e:
            logger.error(f"Local Llama error: {e}")
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
        total_tokens = input_tokens + output_tokens
        seconds = total_tokens / 100
        hours = seconds / 3600
        return hours * self.GPU_COST_PER_HOUR.get("llama-3.2-8b", 0.15)


class AbliteratedProvider(ModelProvider):
    """
    Abliterated/uncensored models via vLLM endpoint.

    Requires running your own vLLM/Ollama server with abliterated models.
    These are NOT available on HuggingFace serverless API.

    Set ABLITERATED_ENDPOINT env var (default: http://localhost:8001)

    To run locally with vLLM:
        vllm serve huihui-ai/Huihui-GLM-4.7-Flash-abliterated --port 8001

    Or with Ollama:
        ollama run huihui/glm4-abliterated
    """

    MODELS = {
        # Popular abliterated models
        "abliterated-glm4": "huihui-ai/Huihui-GLM-4.7-Flash-abliterated",
        "abliterated-llama-70b": "mlabonne/Meta-Llama-3.1-70B-Instruct-abliterated",
        "abliterated-llama-8b": "mlabonne/Llama-3.2-3B-Instruct-abliterated",
        "abliterated-gpt-120b": "ArliAI/gpt-oss-120b-Derestricted",
    }

    def __init__(self, endpoint: Optional[str] = None):
        self.endpoint = endpoint or os.getenv("ABLITERATED_ENDPOINT", "http://localhost:8001")
        self._available = None

    async def is_available(self) -> bool:
        """Check if abliterated endpoint is reachable."""
        if self._available is not None:
            return self._available

        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(
                    f"{self.endpoint}/health",

                ) as resp:
                    self._available = resp.status == 200
        except Exception:
            try:
                async with aiohttp.ClientSession() as session:
                    async with session.get(
                        f"{self.endpoint}/v1/models",

                    ) as resp:
                        self._available = resp.status == 200
            except Exception:
                self._available = False

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
                error=f"Abliterated endpoint not available at {self.endpoint}",
            )

        try:
            model_id = self.MODELS.get(request.model, request.model)

            messages = request.messages.copy()
            if request.system:
                messages.insert(0, {"role": "system", "content": request.system})

            payload = {
                "model": model_id,
                "messages": messages,
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
            }

            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f"{self.endpoint}/v1/chat/completions",
                    json=payload,

                ) as resp:
                    if resp.status != 200:
                        error_text = await resp.text()
                        return ModelResponse(
                            content="",
                            model=request.model,
                            input_tokens=0,
                            output_tokens=0,
                            latency_ms=(time.time() - start_time) * 1000,
                            cost=0,
                            success=False,
                            error=f"Abliterated endpoint error {resp.status}: {error_text}",
                        )

                    data = await resp.json()

            latency = (time.time() - start_time) * 1000
            input_tokens = data.get("usage", {}).get("prompt_tokens", 0)
            output_tokens = data.get("usage", {}).get("completion_tokens", 0)
            content = data["choices"][0]["message"]["content"]

            return ModelResponse(
                content=content,
                model=request.model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                latency_ms=latency,
                cost=self.estimate_cost(input_tokens, output_tokens),
                success=True,
            )

        except Exception as e:
            logger.error(f"Abliterated endpoint error: {e}")
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
        total_tokens = input_tokens + output_tokens
        seconds = total_tokens / 80
        hours = seconds / 3600
        return hours * 1.10
