// Collect task-quality evidence through Brama's stateless provider routes.
// Jeden remains the only agent runtime.

import crypto from 'node:crypto';

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

function validatedRouterBaseURL(raw) {
  const url = new URL(raw);
  const loopback = ['localhost', '127.0.0.1', '::1', '[::1]'].includes(url.hostname);
  const insecureLoopbackAllowed =
    process.env.BRAMA_ALLOW_INSECURE_LOOPBACK?.trim() === '1';
  if (
    (url.protocol !== 'https:' &&
      !(url.protocol === 'http:' && loopback && insecureLoopbackAllowed)) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    (url.pathname !== '' && url.pathname !== '/')
  ) {
    throw new Error('BRAMA_URL must be HTTPS or explicitly enabled loopback HTTP');
  }
  return url.origin;
}

function loadConfig() {
  const config = {
    BRAMA_URL: process.env.BRAMA_URL,
    BRAMA_OPERATIONS_MODEL_ROUTER_TOKEN: process.env.BRAMA_OPERATIONS_MODEL_ROUTER_TOKEN,
    WISENT_APP_AGENT_ID: process.env.WISENT_APP_AGENT_ID,
    WISENT_APP_AGENT_AUTH_SECRET: process.env.WISENT_APP_AGENT_AUTH_SECRET,
    MR_SUPABASE_URL: process.env.MR_SUPABASE_URL,
    MR_SUPABASE_SERVICE_ROLE_KEY: process.env.MR_SUPABASE_SERVICE_ROLE_KEY,
  };
  for (const [key, value] of Object.entries(config)) {
    if (!value) throw new Error(`missing environment variable ${key}`);
  }
  config.BRAMA_URL = validatedRouterBaseURL(config.BRAMA_URL);
  return config;
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
    authorization: `Bearer ${cfg.BRAMA_OPERATIONS_MODEL_ROUTER_TOKEN}`,
  };
}

async function activeModels(cfg) {
  const res = await fetch(`${cfg.BRAMA_URL}/v1/models`, {
    headers: {
      ...sign(cfg, ''),
      'x-jeden-schema-min': '1',
    },
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`read Brama catalog -> ${res.status} ${text}`);
  const catalog = JSON.parse(text);
  return (catalog.models || [])
    .filter((model) => model.available && String(model.id || '').includes('/'))
    .map((model) => String(model.id));
}

async function callModel(cfg, model, args) {
  const body = JSON.stringify({
    model,
    messages: [{ role: 'user', content: args.prompt }],
    max_tokens: 96,
    temperature: 0,
  });
  const started = Date.now();
  const res = await fetch(`${cfg.BRAMA_URL}/v1/chat/completions`, {
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
    provider: model.split('/', 1)[0] || 'unknown',
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
