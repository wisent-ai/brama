# Stream one completion

**Goal:** receive one generation as server-sent events, in whichever of the three chat formats the caller already speaks.

**Status:** written against the streaming contract in [`CORE.md`](../../CORE.md); not re-verified against the newest published release.

**Risk:** credentialed, provider-facing, and potentially billable. A stream that has committed cannot be cancelled without cost: quota already spent stays spent.

**Environment:** running Brama over approved HTTPS or authenticated loopback.

**Preconditions:** `BRAMA_URL`; dedicated bearer in `BRAMA_BEARER`; an allowed alias, canonical `provider/model` route, or selector. Subscription routes additionally need the three agent HMAC headers over the exact raw body, exactly as the buffered path does.

**Inputs:** synthetic non-sensitive text only.

**Artifacts and side effects:** one provider generation, ledger counters for the subscription that paid, and process telemetry. No file is written by the gateway.

## OpenAI chat completions

```bash
umask 077
cat >stream.json <<'JSON'
{"model":"wisent-backend/chat/primary","messages":[{"role":"user","content":"Count from one to five."}],"max_tokens":64,"temperature":0,"stream":true}
JSON
```

After explicit provider-cost approval:

```bash
curl --fail --silent --show-error --no-buffer \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H 'Content-Type: application/json' \
  --data-binary @stream.json \
  "$BRAMA_URL/v1/chat/completions"
```

Expected: `Content-Type: text/event-stream`, then `data:` frames whose `object` is `chat.completion.chunk` — one carrying `delta.role`, then `delta.content` fragments, then a frame with `finish_reason`, then a frame carrying `usage` when the provider published a token meter, and finally `data: [DONE]`. Provider text is intentionally not predicted here.

## Anthropic Messages

```bash
cat >stream-anthropic.json <<'JSON'
{"model":"claude-code/claude-sonnet-4-6","messages":[{"role":"user","content":"Count from one to five."}],"max_tokens":64,"stream":true}
JSON
```

```bash
curl --fail --silent --show-error --no-buffer \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H "x-agent-id: $BRAMA_AGENT_ID" \
  -H "x-agent-timestamp: $BRAMA_AGENT_TIMESTAMP" \
  -H "x-agent-signature: $BRAMA_AGENT_SIGNATURE" \
  -H 'Content-Type: application/json' \
  --data-binary @stream-anthropic.json \
  "$BRAMA_URL/v1/messages"
```

Expected event order: `message_start`, `content_block_start`, one or more `content_block_delta`, `content_block_stop`, `message_delta` carrying `stop_reason` and output tokens, `message_stop`. A tool call appears as its own `tool_use` content block whose arguments arrive as `input_json_delta` fragments.

The signature covers the exact bytes of `stream-anthropic.json`; sign the file, do not re-serialize it.

## OpenAI Responses

```bash
cat >stream-responses.json <<'JSON'
{"model":"codex/gpt-5.3-codex-spark","input":"Count from one to five.","max_output_tokens":64,"stream":true}
JSON
```

```bash
curl --fail --silent --show-error --no-buffer \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H "x-agent-id: $BRAMA_AGENT_ID" \
  -H "x-agent-timestamp: $BRAMA_AGENT_TIMESTAMP" \
  -H "x-agent-signature: $BRAMA_AGENT_SIGNATURE" \
  -H 'Content-Type: application/json' \
  --data-binary @stream-responses.json \
  "$BRAMA_URL/v1/responses"
```

Expected event order: `response.created`, `response.output_item.added`, `response.content_part.added`, `response.output_text.delta` fragments, `response.output_text.done`, `response.content_part.done`, `response.output_item.done`, `response.completed` carrying the whole output again with `usage`.

## Failure path

A refusal that happens **before** the first byte is an ordinary error document with Brama's own contract code and HTTP status: `403 forbidden` for an allowlist miss, `429 subscription_unavailable` when every bounded credential is spent, `503 credential_unauthorized` when the provider refused the grant. Every retry Brama is allowed to make has already happened by then.

A failure **after** commit ends the stream without its terminal event: no `data: [DONE]`, no `message_stop`, and `response.failed` instead of `response.completed`. That is the signal that the generation is incomplete. Brama does not continue it on another credential, because a second attempt would duplicate both cost and emitted text; starting a new request is the caller's decision.

Kimi streams carry no usage numbers. The provider's coding endpoint publishes no token meter on its event stream, so none is invented: the frames simply have no `usage`.

## Cleanup

```bash
rm -f stream.json stream-anthropic.json stream-responses.json
unset BRAMA_BEARER BRAMA_AGENT_ID BRAMA_AGENT_TIMESTAMP BRAMA_AGENT_SIGNATURE
```

Quota consumed by an approved stream cannot be undone, including for a stream the caller abandoned.

## Next

Buffered equivalents are in [`../core/call-http-api.md`](../core/call-http-api.md); agent-owned selection is in [`../core/call-agent-selectors.md`](../core/call-agent-selectors.md).
