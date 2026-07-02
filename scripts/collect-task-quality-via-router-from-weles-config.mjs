// Collect task-quality evidence through the deployed model-router HTTP API.
// This avoids local CLI/path differences when the evidence is meant to drive
// production routing.

import crypto from 'node:crypto';

const WELLES_SUPABASE_URL = process.env.SUPABASE_URL;
const WELLES_SUPABASE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY;

const PROVIDER_MODELS = {
  claude_code: 'claude-code-subscription',
  codex: 'codex-subscription',
  kimi: 'kimi-subscription',
  opencode: 'opencode-subscription',
};

function die(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs() {
  const out = {
    task: '',
    prompt: '',
    expectedContains: '',
    expectedExact: '',
    persist: false,
  };
  for (let i = 2; i < process.argv.length; i += 1) {
    const arg = process.argv[i];
    if (arg === '--task') out.task = process.argv[++i] || '';
    else if (arg === '--prompt') out.prompt = process.argv[++i] || '';
    else if (arg === '--expected-contains') out.expectedContains = process.argv[++i] || '';
    else if (arg === '--expected-exact') out.expectedExact = process.argv[++i] || '';
    else if (arg === '--persist') out.persist = true;
    else throw new Error(`unknown arg: ${arg}`);
  }
  if (!out.task) throw new Error('missing --task');
  if (!out.prompt) throw new Error('missing --prompt');
  if (!out.expectedContains && !out.expectedExact) {
    throw new Error('missing --expected-contains or --expected-exact');
  }
  return out;
}

async function loadConfig() {
  if (!WELLES_SUPABASE_URL || !WELLES_SUPABASE_KEY) {
    die('Set SUPABASE_URL and SUPABASE_SERVICE_ROLE_KEY for Weles');
  }
  const headers = {
    apikey: WELLES_SUPABASE_KEY,
    Authorization: `Bearer ${WELLES_SUPABASE_KEY}`,
  };
  const res = await fetch(
    `${WELLES_SUPABASE_URL}/rest/v1/service_credentials?id=in.(codex-reauth-config,claude-reauth-config)&select=id,metadata`,
    { headers }
  );
  const text = await res.text();
  if (!res.ok) throw new Error(`read Weles config -> ${res.status} ${text}`);
  const rows = JSON.parse(text);
  const configs = new Map(rows.map((row) => [row.id, row.metadata || {}]));
  const meta = configs.get('codex-reauth-config') || configs.get('claude-reauth-config');
  if (!meta) throw new Error('missing codex-reauth-config / claude-reauth-config');
  for (const key of [
    'MODEL_ROUTER_URL',
    'WISENT_APP_AGENT_ID',
    'WISENT_APP_AGENT_AUTH_SECRET',
    'MR_SUPABASE_URL',
    'MR_SUPABASE_SERVICE_ROLE_KEY',
  ]) {
    if (!meta[key]) throw new Error(`config missing ${key}`);
  }
  return meta;
}

function sign(cfg, body) {
  const ts = String(Math.floor(Date.now() / 1000));
  const bodyHash = crypto.createHash('sha256').update(body).digest('hex');
  const msg = `${cfg.WISENT_APP_AGENT_ID}:${ts}:${bodyHash}`;
  const sig = crypto.createHmac('sha256', cfg.WISENT_APP_AGENT_AUTH_SECRET).update(msg).digest('hex');
  return {
    'content-type': 'application/json',
    'x-agent-id': cfg.WISENT_APP_AGENT_ID,
    'x-agent-timestamp': ts,
    'x-agent-signature': sig,
  };
}

async function activeModels(cfg) {
  const headers = {
    apikey: cfg.MR_SUPABASE_SERVICE_ROLE_KEY,
    Authorization: `Bearer ${cfg.MR_SUPABASE_SERVICE_ROLE_KEY}`,
  };
  const url = `${cfg.MR_SUPABASE_URL.replace(/\/+$/, '')}/rest/v1/trade_agent_subscriptions?select=provider&instance_id=eq.${encodeURIComponent(cfg.WISENT_APP_AGENT_ID)}&status=eq.active`;
  const res = await fetch(url, { headers });
  const text = await res.text();
  if (!res.ok) throw new Error(`read active subscriptions -> ${res.status} ${text}`);
  const rows = JSON.parse(text);
  const models = [];
  for (const row of rows) {
    const model = PROVIDER_MODELS[row.provider];
    if (model && !models.includes(model)) models.push(model);
  }
  return models;
}

async function callModel(cfg, model, args) {
  const body = JSON.stringify({
    model,
    messages: [{ role: 'user', content: args.prompt }],
    max_tokens: 96,
    temperature: 0,
  });
  const started = Date.now();
  const res = await fetch(`${cfg.MODEL_ROUTER_URL.replace(/\/+$/, '')}/v1/chat/completions`, {
    method: 'POST',
    headers: sign(cfg, body),
    body,
  });
  const latencyMs = Date.now() - started;
  const text = await res.text();
  let data;
  try { data = JSON.parse(text); } catch { data = { raw: text }; }
  const content = data?.choices?.[0]?.message?.content || '';
  const expectedOk = args.expectedExact
    ? content.trim() === args.expectedExact.trim()
    : content.includes(args.expectedContains);
  const passed = res.ok && expectedOk;
  return {
    model,
    provider: Object.entries(PROVIDER_MODELS).find(([, value]) => value === model)?.[0] || 'unknown',
    status: passed ? 'active' : 'failed',
    score: passed ? 1.0 : 0.0,
    ok: res.ok,
    content,
    latencyMs,
    error: data?.error?.message || data?.error || null,
  };
}

function rowFor(cfg, args, result) {
  const now = new Date().toISOString();
  return {
    agent_id: cfg.WISENT_APP_AGENT_ID,
    source: 'model-router-task-quality',
    provider: result.provider,
    service: serviceName(result.provider),
    subscription_id: null,
    account_identifier: args.task,
    status: result.status,
    auth_method: null,
    plan: null,
    check_kind: 'task_quality',
    confidence: 'observed',
    error: result.error,
    metadata: {
      task: args.task,
      model: result.model,
      score: result.score,
      prompt: args.prompt,
      expectedExact: args.expectedExact || null,
      expectedContains: args.expectedContains || null,
      output: String(result.content).slice(0, 1500),
      latencyMs: result.latencyMs,
      success: result.ok,
      collectedVia: 'model-router-http',
    },
    checked_at: now,
    updated_at: now,
  };
}

async function persistRows(cfg, args, rows) {
  const headers = {
    apikey: cfg.MR_SUPABASE_SERVICE_ROLE_KEY,
    Authorization: `Bearer ${cfg.MR_SUPABASE_SERVICE_ROLE_KEY}`,
    'content-type': 'application/json',
  };
  const base = `${cfg.MR_SUPABASE_URL.replace(/\/+$/, '')}/rest/v1/subscription_router_checks`;
  const deleteUrl = `${base}?agent_id=eq.${encodeURIComponent(cfg.WISENT_APP_AGENT_ID)}&source=eq.model-router-task-quality&account_identifier=eq.${encodeURIComponent(args.task)}`;
  const del = await fetch(deleteUrl, { method: 'DELETE', headers });
  const delText = await del.text();
  if (!del.ok) throw new Error(`delete old task quality rows -> ${del.status} ${delText}`);
  if (!rows.length) return;
  const ins = await fetch(base, {
    method: 'POST',
    headers: { ...headers, Prefer: 'return=representation' },
    body: JSON.stringify(rows),
  });
  const insText = await ins.text();
  if (!ins.ok) throw new Error(`insert task quality rows -> ${ins.status} ${insText}`);
}

function serviceName(provider) {
  if (provider === 'claude_code') return 'Claude Code';
  if (provider === 'codex') return 'Codex';
  if (provider === 'kimi') return 'Kimi Code';
  if (provider === 'opencode') return 'OpenCode';
  return provider;
}

async function main() {
  const args = parseArgs();
  const cfg = await loadConfig();
  const models = await activeModels(cfg);
  const results = [];
  for (const model of models) {
    results.push(await callModel(cfg, model, args));
  }
  const rows = results.map((result) => rowFor(cfg, args, result));
  if (args.persist) await persistRows(cfg, args, rows);
  const activeRows = rows.filter((row) => row.status === 'active');
  const topScore = activeRows.length
    ? Math.max(...activeRows.map((row) => row.metadata.score))
    : null;
  const bestModels = topScore === null
    ? []
    : activeRows
        .filter((row) => row.metadata.score === topScore)
        .map((row) => row.metadata.model);
  console.log(JSON.stringify({
    ok: true,
    task: args.task,
    persisted: args.persist,
    rows: rows.length,
    bestModel: bestModels.length === 1 ? bestModels[0] : null,
    bestModels,
    results: rows.map((row) => ({
      model: row.metadata.model,
      status: row.status,
      score: row.metadata.score,
      output: row.metadata.output,
      error: row.error,
    })),
  }, null, 2));
}

main().catch((error) => die(error.stack || error.message || String(error)));
