"""
Model Router - CLI test entry point.

Run with: python -m model_router
"""

import asyncio

from .detection import detect_compute_resources, select_model_for_resources
from .router import ModelRouter


async def test():
    print("=" * 60)
    print("MODEL ROUTER TEST")
    print("=" * 60)

    # Detect local compute resources
    print("\n1. Detecting local compute resources...")
    resources = detect_compute_resources()
    print(f"   GPU Type: {resources.gpu_type or 'None (CPU only)'}")
    print(f"   GPU Name: {resources.gpu_name or 'N/A'}")
    print(f"   VRAM: {resources.vram_gb:.1f} GB")
    print(f"   RAM: {resources.ram_gb:.1f} GB")
    print(f"   CPU Cores: {resources.cpu_cores}")
    print(f"   CUDA: {'Yes' if resources.has_cuda else 'No'}")
    print(f"   Metal: {'Yes' if resources.has_metal else 'No'}")

    # Show recommended model
    model, backend = select_model_for_resources(resources)
    print(f"\n   Recommended local model: {model}")
    print(f"   Backend: {backend}")

    # Initialize router
    print("\n2. Initializing model router...")
    router = ModelRouter(auto_start_local=True)

    print("\n3. Checking provider availability...")
    availability = await router.get_available_providers()
    for provider, available in availability.items():
        status = "OK" if available else "UNAVAILABLE"
        print(f"   {provider}: {status}")

    print("\n4. Testing inference (will auto-start local if needed)...")
    response = await router.complete(
        messages=[{"role": "user", "content": "Say hello in 10 words or less."}],
        model="claude-haiku-3.5",
    )

    print(f"\n5. Response:")
    print(f"   Model used: {response.model}")
    print(f"   Success: {response.success}")
    if response.success:
        print(f"   Content: {response.content}")
        print(f"   Tokens: {response.input_tokens} in, {response.output_tokens} out")
        print(f"   Cost: ${response.cost:.6f}")
        print(f"   Latency: {response.latency_ms:.1f}ms")
    else:
        print(f"   Error: {response.error}")

    print(f"\n6. Router stats: {router.get_stats()}")

    # Check if local inference was started
    if router.local_inference_manager.is_running():
        print(f"\n7. Local inference is running:")
        print(f"   Model: {router.local_inference_manager.model}")
        print(f"   Backend: {router.local_inference_manager.backend}")
        print(f"   Port: {router.local_inference_manager.port}")

        print("\n   Stopping local inference...")
        router.local_inference_manager.stop()

    print("\n" + "=" * 60)
    print("TEST COMPLETE")
    print("=" * 60)

asyncio.run(test())
