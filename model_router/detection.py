"""
Model Router detection - hardware detection, model selection, and local inference management.
"""

import os
import sys
import asyncio
import subprocess
import platform
import shutil
import aiohttp
import logging
from typing import Optional, Tuple

from .types import ComputeResources

logger = logging.getLogger(__name__)


def detect_compute_resources() -> ComputeResources:
    """Detect available local compute resources."""
    import psutil
    resources = ComputeResources(
        ram_gb=psutil.virtual_memory().total / (1024**3),
        cpu_cores=psutil.cpu_count(logical=False) or psutil.cpu_count() or 1,
    )
    # Check for Apple Silicon
    if platform.system() == "Darwin" and platform.machine() == "arm64":
        resources.gpu_type = "apple_silicon"
        resources.has_metal = True
        resources.vram_gb = resources.ram_gb
        resources.gpu_name = f"Apple {platform.processor()}"
    # Check for NVIDIA GPU
    if shutil.which("nvidia-smi"):
        try:
            result = subprocess.run(
                ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader,nounits"],
                capture_output=True, text=True
            )
            if result.returncode == 0 and result.stdout.strip():
                parts = result.stdout.strip().split("\n")[0].split(", ")
                if len(parts) >= 2:
                    resources.gpu_type = "nvidia"
                    resources.gpu_name = parts[0].strip()
                    resources.vram_gb = float(parts[1].strip()) / 1024
                    resources.has_cuda = True
        except Exception as e:
            logger.debug(f"nvidia-smi check failed: {e}")
    # Check for AMD GPU (ROCm)
    if shutil.which("rocm-smi"):
        try:
            result = subprocess.run(
                ["rocm-smi", "--showmeminfo", "vram"], capture_output=True, text=True
            )
            if result.returncode == 0:
                resources.gpu_type = "amd"
                resources.gpu_name = "AMD GPU"
                for line in result.stdout.split("\n"):
                    if "Total" in line:
                        for p in line.split():
                            if p.isdigit():
                                resources.vram_gb = float(p) / (1024**3)
                                break
        except Exception as e:
            logger.debug(f"rocm-smi check failed: {e}")
    return resources


def select_model_for_resources(resources: ComputeResources) -> Tuple[str, str]:
    """Select best model for available resources. Returns (model_name, backend)."""
    if resources.gpu_type == "apple_silicon":
        if resources.vram_gb >= 64:
            return ("mlx-community/Qwen3-32B-Instruct-4bit", "mlx")
        elif resources.vram_gb >= 32:
            return ("mlx-community/gemma-3-12b-it-4bit", "mlx")
        elif resources.vram_gb >= 16:
            return ("mlx-community/DeepSeek-R1-0528-Qwen3-8B-4bit", "mlx")
        elif resources.vram_gb >= 8:
            return ("mlx-community/Qwen3-4B-Instruct-4bit", "mlx")
        else:
            return ("mlx-community/Qwen3-1.7B-Instruct-4bit", "mlx")
    elif resources.has_cuda:
        if resources.vram_gb >= 80:
            return ("Qwen/Qwen3-32B-Instruct", "vllm")
        elif resources.vram_gb >= 24:
            return ("google/gemma-3-12b-it", "vllm")
        elif resources.vram_gb >= 16:
            return ("deepseek-ai/DeepSeek-R1-0528-Qwen3-8B", "vllm")
        elif resources.vram_gb >= 8:
            return ("Qwen/Qwen3-4B-Instruct", "vllm")
        else:
            return ("Qwen/Qwen3-1.7B-Instruct", "vllm")
    else:
        if resources.ram_gb >= 32:
            return ("unsloth/DeepSeek-R1-0528-Qwen3-8B-GGUF", "llama-cpp")
        elif resources.ram_gb >= 16:
            return ("Qwen/Qwen3-4B-Instruct-GGUF", "llama-cpp")
        else:
            return ("Qwen/Qwen3-1.7B-Instruct-GGUF", "llama-cpp")


class LocalInferenceManager:
    """Manages local inference server lifecycle."""

    def __init__(self):
        self.process: Optional[subprocess.Popen] = None
        self.model: Optional[str] = None
        self.backend: Optional[str] = None
        self.port: int = 8000
        self.resources: Optional[ComputeResources] = None

    def get_resources(self) -> ComputeResources:
        if self.resources is None:
            self.resources = detect_compute_resources()
        return self.resources

    def is_running(self) -> bool:
        if self.process is None:
            return False
        return self.process.poll() is None

    async def start(self, model: Optional[str] = None, backend: Optional[str] = None) -> bool:
        """Start local inference server based on available resources."""
        if self.is_running():
            logger.info("Local inference server already running")
            return True
        resources = self.get_resources()
        logger.info(f"Detected resources: {resources.gpu_type or 'CPU'}, "
                   f"{resources.vram_gb:.1f}GB VRAM, {resources.ram_gb:.1f}GB RAM")
        if model is None or backend is None:
            model, backend = select_model_for_resources(resources)
        self.model = model
        self.backend = backend
        logger.info(f"Starting local inference: {model} with {backend}")
        try:
            if backend == "mlx":
                return await self._start_mlx(model)
            elif backend == "vllm":
                return await self._start_vllm(model)
            elif backend == "llama-cpp":
                return await self._start_llamacpp(model)
            else:
                logger.error(f"Unknown backend: {backend}")
                return False
        except Exception as e:
            logger.error(f"Failed to start local inference: {e}")
            return False

    async def _start_mlx(self, model: str) -> bool:
        try:
            import mlx_lm  # noqa: F401
        except ImportError:
            logger.info("Installing mlx-lm...")
            subprocess.run([sys.executable, "-m", "pip", "install", "mlx-lm"], check=True)
        cmd = [sys.executable, "-m", "mlx_lm.server", "--model", model, "--port", str(self.port)]
        self.process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return await self._wait_for_server()

    async def _start_vllm(self, model: str) -> bool:
        try:
            import vllm  # noqa: F401
        except ImportError:
            logger.info("Installing vllm...")
            subprocess.run([sys.executable, "-m", "pip", "install", "vllm"], check=True)
        cmd = [sys.executable, "-m", "vllm.entrypoints.openai.api_server",
               "--model", model, "--port", str(self.port), "--trust-remote-code"]
        resources = self.get_resources()
        if resources.vram_gb < 24:
            cmd.extend(["--quantization", "awq"])
        self.process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return await self._wait_for_server()

    async def _start_llamacpp(self, model: str) -> bool:
        try:
            from llama_cpp import Llama  # noqa: F401
        except ImportError:
            logger.info("Installing llama-cpp-python...")
            subprocess.run([sys.executable, "-m", "pip", "install", "llama-cpp-python[server]"], check=True)
        cmd = [sys.executable, "-m", "llama_cpp.server",
               "--model", model, "--port", str(self.port), "--n_ctx", "4096"]
        self.process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return await self._wait_for_server()

    async def _wait_for_server(self) -> bool:
        """Wait for server to become available. Waits as long as process is alive."""
        check_count = 0
        while True:
            if self.process and self.process.poll() is not None:
                stderr = self.process.stderr.read().decode() if self.process.stderr else ""
                logger.error(f"Local inference server died: {stderr}")
                return False
            try:
                async with aiohttp.ClientSession() as session:
                    async with session.get(f"http://localhost:{self.port}/v1/models") as resp:
                        if resp.status == 200:
                            logger.info(f"Local inference server ready on port {self.port}")
                            return True
            except Exception:
                pass
            check_count += 1
            if check_count % 30 == 0:
                logger.info(f"Still waiting for model to load... ({check_count * 2}s elapsed)")
            await asyncio.sleep(2)

    def stop(self):
        """Stop local inference server."""
        if self.process:
            self.process.terminate()
            try:
                self.process.wait()
            except subprocess.TimeoutExpired:
                self.process.kill()
            self.process = None
            logger.info("Local inference server stopped")


_local_inference: Optional[LocalInferenceManager] = None

def get_local_inference_manager() -> LocalInferenceManager:
    """Get global local inference manager."""
    global _local_inference
    if _local_inference is None:
        _local_inference = LocalInferenceManager()
    return _local_inference
