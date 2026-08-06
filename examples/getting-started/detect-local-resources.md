# Detect local resources

**Goal:** obtain Brama's local hardware report and external-runtime model recommendation.

**Status:** written against the `0.1.0` contract and not re-verified against the published `0.2.5`.

**Risk:** local read-only. No credential, service, product state, provider network, or model cost.

**Environment:** clean macOS or Linux shell in a Brama source checkout.

**Preconditions:** Git and the Rust toolchain compatible with `Cargo.lock`.

**Inputs:** none. Host hardware is observed locally.

**Artifacts and side effects:** Cargo may create `target/` build cache. Brama creates no state and starts no background process.

## Steps

```bash
git clone https://github.com/wisent-ai/brama.git brama
cd brama
cargo run --locked -- detect
```

## Observable result

Output contains host-specific values under these stable labels:

```text
GPU Type: ...
GPU Name: ...
VRAM: ... GB
RAM: ... GB
CPU Cores: ...
CUDA: ...
Metal: ...
Recommended model: ...
Recommended backend: ...
```

The result is the complete resource report, not merely a successful process exit.

## Failure path

A missing Rust toolchain or native system command fails before a complete report. Install the prerequisite named by the error; do not add provider credentials or start the gateway as a workaround.

## Cleanup

No process cleanup is required. Remove the checkout to remove source and build cache.

## Next

Continue with [`../operations/inspect-build-and-health.md`](../operations/inspect-build-and-health.md).
