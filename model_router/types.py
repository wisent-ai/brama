"""
Model Router types - dataclasses, base classes, and constants.
"""

import time
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, Dict, List, Optional


@dataclass
class ComputeResources:
    """Local compute resources available."""
    gpu_type: Optional[str] = None  # "nvidia", "apple_silicon", "amd", None
    gpu_name: Optional[str] = None
    vram_gb: float = 0
    ram_gb: float = 0
    cpu_cores: int = 0
    has_cuda: bool = False
    has_metal: bool = False


@dataclass
class ModelRequest:
    """Request to a model."""
    messages: List[Dict[str, str]]
    model: str
    max_tokens: int = 4096
    temperature: float = 0.7
    stream: bool = False
    system: Optional[str] = None


@dataclass
class ModelResponse:
    """Response from a model."""
    content: str
    model: str
    input_tokens: int
    output_tokens: int
    latency_ms: float
    cost: float
    success: bool
    error: Optional[str] = None


class ModelProvider(ABC):
    """Abstract base for model providers."""

    @abstractmethod
    async def complete(self, request: ModelRequest) -> ModelResponse:
        """Generate completion."""
        pass

    @abstractmethod
    def estimate_cost(self, input_tokens: int, output_tokens: int) -> float:
        """Estimate cost for tokens."""
        pass

    @abstractmethod
    async def is_available(self) -> bool:
        """Check if provider is available."""
        pass
