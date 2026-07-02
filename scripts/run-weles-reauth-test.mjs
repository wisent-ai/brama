import { readFile } from 'node:fs/promises';

function parseEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(line.trim());
    if (!match) continue;
    let value = match[2].trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    out[match[1]] = value.replace(/\\n/g, '').trim();
  }
  return out;
}

function parseArgs() {
  const out = {
    action: '',
    timeoutMs: 900_000,
  };
  for (let i = 2; i < process.argv.length; i += 1) {
    const arg = process.argv[i];
    if (arg === '--action') out.action = process.argv[++i] || '';
    else if (arg === '--timeout-ms') out.timeoutMs = Number(process.argv[++i] || '');
    else throw new Error(`unknown arg: ${arg}`);
  }
  if (!/^(codex_reauth|kimi_reauth|claude_reauth)$/.test(out.action)) {
    throw new Error('usage: node scripts/run-weles-reauth-test.mjs --action codex_reauth|kimi_reauth|claude_reauth');
  }
  if (!Number.isFinite(out.timeoutMs) || out.timeoutMs < 60_000) {
    throw new Error('timeout must be at least 60000ms');
  }
  return out;
}

async function loadWelesEnv() {
  const env = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));
  const base = (env.NEXT_PUBLIC_BASE_URL || 'https://weles.wisent.com').replace(/\/+$/, '');
  const token = env.WELES_DIAG_API_TOKEN || env.WELES_API_TOKEN;
  if (!token) throw new Error('missing Weles API token');
  return { base, token };
}

async function loadAgentMetadata() {
  const env = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));
  const supabaseUrl = env.NEXT_PUBLIC_SUPABASE_URL || env.SUPABASE_URL;
  const serviceRole = env.SUPABASE_SERVICE_ROLE_KEY;
  if (!supabaseUrl || !serviceRole) throw new Error('missing Weles Supabase service config');
  const res = await fetch(`${supabaseUrl.replace(/\/+$/, '')}/rest/v1/service_credentials?id=in.(codex-reauth-config,claude-reauth-config,kimi-reauth-config)&select=id,metadata`, {
    headers: {
      apikey: serviceRole,
      Authorization: `Bearer ${serviceRole}`,
    },
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`read Weles reauth config -> ${res.status} ${text.slice(0, 500)}`);
  const rows = JSON.parse(text);
  const configs = new Map(rows.map((row) => [row.id, row.metadata || {}]));
  const meta = configs.get('codex-reauth-config') || configs.get('claude-reauth-config') || configs.get('kimi-reauth-config') || {};
  return {
    agentId: String(meta.WISENT_APP_AGENT_ID || 'unknown-agent'),
  };
}

function providerForAction(action) {
  if (action === 'codex_reauth') return { provider: 'codex', model: 'codex-subscription' };
  if (action === 'kimi_reauth') return { provider: 'kimi', model: 'kimi-subscription' };
  return { provider: 'claude_code', model: 'claude-code-subscription' };
}

function summarize(row) {
  return {
    id: row.id,
    action: row.action,
    status: row.status,
    scheduled_at: row.scheduled_at,
    started_at: row.started_at,
    completed_at: row.completed_at,
    claimed_by: row.claimed_by,
    error: row.error || null,
    result_keys: row.result && typeof row.result === 'object' ? Object.keys(row.result) : [],
  };
}

async function main() {
  const args = parseArgs();
  const { base, token } = await loadWelesEnv();
  const { agentId } = await loadAgentMetadata();
  const { provider, model } = providerForAction(args.action);
  const idempotency = `manual-reauth-test:${args.action}:${Date.now()}`;
  const createBody = {
    action: args.action,
    params: {
      source: 'manual-model-router-test',
      reason: 'explicit_reauth_path_test',
      agent_id: agentId,
      provider,
      model,
      failed_subscription_id: `manual-test-${Date.now()}`,
    },
    idempotency_key: idempotency,
    priority: 100,
  };

  const createRes = await fetch(`${base}/api/v1/runs`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(createBody),
  });
  const createText = await createRes.text();
  let createJson;
  try { createJson = JSON.parse(createText); } catch { createJson = { raw: createText }; }
  if (!createRes.ok) {
    console.log(JSON.stringify({ ok: false, phase: 'create', status: createRes.status, body: createJson }, null, 2));
    process.exit(1);
  }

  const runId = createJson.row?.id || createJson.id;
  if (!runId) throw new Error(`create response missing row id: ${createText.slice(0, 500)}`);
  console.log(JSON.stringify({ ok: true, phase: 'created', action: args.action, row: summarize(createJson.row || createJson) }, null, 2));

  const started = Date.now();
  let last = createJson.row || createJson;
  while (Date.now() - started < args.timeoutMs) {
    if (['completed', 'succeeded', 'success', 'done', 'failed', 'error', 'cancelled', 'canceled'].includes(String(last.status))) {
      const ok = ['completed', 'succeeded', 'success', 'done'].includes(String(last.status));
      console.log(JSON.stringify({ ok, phase: 'terminal', row: summarize(last) }, null, 2));
      process.exit(ok ? 0 : 1);
    }
    await new Promise((resolve) => setTimeout(resolve, 10_000));
    const pollRes = await fetch(`${base}/api/v1/runs/${encodeURIComponent(runId)}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    const pollText = await pollRes.text();
    let pollJson;
    try { pollJson = JSON.parse(pollText); } catch { pollJson = { raw: pollText }; }
    if (!pollRes.ok) {
      console.log(JSON.stringify({ ok: false, phase: 'poll', status: pollRes.status, body: pollJson }, null, 2));
      process.exit(1);
    }
    last = pollJson;
    console.log(JSON.stringify({ ok: true, phase: 'poll', elapsedMs: Date.now() - started, row: summarize(last) }, null, 2));
  }
  console.log(JSON.stringify({ ok: false, phase: 'timeout', row: summarize(last) }, null, 2));
  process.exit(1);
}

main().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exit(1);
});
