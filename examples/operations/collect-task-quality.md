# Run bounded provider diagnostics and task-quality collection

**Goal:** diagnose one route or refresh evidence used by `task:<name>` selection.

**Status:** written against the `0.1.0` contract and not re-verified against the newest published release.

**Risk:** credentialed, provider-facing, and billable. `--persist` also appends non-secret task-quality state. Explicit human approval must name agent, task, prompt class, expected result, model cap, persistence, and spend.

**Environment:** Brama binary with the local capability broker available.

**Preconditions:** exact agent subscription metadata and provider capabilities; owner-approved synthetic prompt; known cleanup/retention policy.

**Inputs:** task name, prompt, exact/substring expectation, bounded model count. Never use production prompts or personal data for qualification.

**Artifacts and side effects:** each selected model can consume one provider call; output records attempts and latency. `--persist` appends quality observations to `BRAMA_STATE_DIR/journal.jsonl`.

## One route diagnostic

After explicit approval:

```bash
brama test \
  --model openai/default \
  --agent-id wisent-app \
  --allow-provider-cost
```

This command refuses to call the provider unless `--allow-provider-cost` is present. Expected output is a normalized response or actionable provider/auth failure; provider content is not predicted here.

## Bounded quality collection

First run without persistence; this still contacts providers and costs quota:

```bash
brama collect-task-quality \
  --agent-id wisent-app \
  --task smoke-exact \
  --prompt 'Reply with the single word ready.' \
  --expected-exact ready \
  --max-models 3 \
  --allow-provider-cost
```

Expected JSON includes `task`, `maxModels`, `rows`, and `bestModel`. Every row includes route, status, score, latency, attempts, and success.

Only after reviewing that output and obtaining separate state-mutation approval, repeat with `--persist`. Persisted observations change future `task:smoke-exact` ranking.

## Failure and recovery

Missing cost acknowledgment fails before provider execution. Empty task/prompt, conflicting expectations, or zero model count is `invalid_request`. Provider/dependency failures remain bounded by the selected model cap and per-request attempt cap. Do not broaden the cap to hide failures; repair capability, ownership, route, or provider state.

## Cleanup

A non-persisted run needs no state cleanup; quota cannot be restored. Persistent observations are append-only evidence. Do not edit the journal manually; supersede stale evidence through an approved new collection or restore the owning state backup.

## Next

Inspect secret-free counters through [`inspect-build-and-health.md`](inspect-build-and-health.md).
