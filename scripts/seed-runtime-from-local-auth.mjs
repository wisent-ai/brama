// Seed model-router runtime subscriptions from local CLI auth files.
//
// Reads router config from Weles service_credentials metadata, then POSTs to
// the existing /v1/subscriptions/{agent} API so encryption stays owned by the
// router. This does not mutate old revoked rows.

import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';

const WELLES_SUPABASE_URL = process.env.SUPABASE_URL;
const WELLES_SUPABASE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY;

function die(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs() {
  const args = {
    providers: new Set(['codex', 'kimi']),
    force: false,
    codexAuth: join(homedir(), '.codex', 'auth.json'),
    kimiAuth: join(homedir(), '.kimi-code', 'credentials', 'kimi-code.json'),
  };
  for (let i = 2; i < process.argv.length; i += 1) {
    const arg = process.argv[i];
    if (arg === '--provider') {
      const provider = process.argv[++i] || '';
      if (!['codex', 'kimi', 'all'].includes(provider)) {
        throw new Error(`invalid --provider ${provider}`);
      }
      args.providers = provider === 'all' ? new Set(['codex', 'kimi']) : new Set([provider]);
    } else if (arg === '--force') {
      args.force = true;
    } else if (arg === '--codex-auth') {
      args.codexAuth = process.argv[++i] || '';
    } else if (arg === '--kimi-auth') {
      args.kimiAuth = process.argv[++i] || '';
    } else {
      throw new Error(`unknown arg: ${arg}`);
    }
  }
  return args;
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
    { headers },
  );
  const text = await res.text();
  if (!res.ok) throw new Error(`read Weles config -> ${res.status} ${text}`);
  const rows = JSON.parse(text);
  const configs = new Map(rows.map((row) => [row.id, row.metadata || {}]));
  const meta = configs.get('codex-reauth-config') || configs.get('claude-reauth-config');
  if (!meta) throw new Error('missing codex-reauth-config / claude-reauth-config');
  for (const key of ['MODEL_ROUTER_URL', 'WISENT_APP_AGENT_ID']) {
    if (!meta[key]) throw new Error(`config missing ${key}`);
  }
  return {
    routerUrl: String(meta.MODEL_ROUTER_URL).replace(/\/+$/, ''),
    agentId: String(meta.WISENT_APP_AGENT_ID),
  };
}

async function loadAuthJson(path, label) {
  const raw = await readFile(path, 'utf8');
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error(`${label} auth is not valid JSON at ${path}: ${error.message}`);
  }
  if (label === 'kimi') {
    const accessLen = typeof parsed.access_token === 'string' ? parsed.access_token.length : 0;
    const refreshLen = typeof parsed.refresh_token === 'string' ? parsed.refresh_token.length : 0;
    if (accessLen <= 32 || refreshLen <= 32) {
      throw new Error(
        `${label} auth at ${path} has empty OAuth tokens; run kimi login or pull a fresh non-empty credentials blob first`,
      );
    }
  }
  return raw;
}

async function activeProviders(cfg) {
  const res = await fetch(`${cfg.routerUrl}/v1/subscriptions/${encodeURIComponent(cfg.agentId)}`);
  const text = await res.text();
  if (!res.ok) throw new Error(`read active subscriptions -> ${res.status} ${text}`);
  const data = JSON.parse(text);
  const rows = Array.isArray(data.subscriptions) ? data.subscriptions : [];
  return new Set(rows.map((row) => row.provider).filter(Boolean));
}

async function seedProvider(cfg, provider, apiKey, force) {
  const active = await activeProviders(cfg);
  if (active.has(provider) && !force) {
    return { provider, skipped: true, reason: 'active row already present' };
  }

  const labelKind = provider === 'codex' ? 'auth-json' : 'credentials-json';
  const body = JSON.stringify({
    user_id: 'local-cli-seed',
    provider,
    api_key: apiKey,
    label: `local-cli-seed ${provider} ${labelKind} ${new Date().toISOString()}`,
  });
  const res = await fetch(`${cfg.routerUrl}/v1/subscriptions/${encodeURIComponent(cfg.agentId)}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body,
  });
  const text = await res.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    data = { raw: text };
  }
  if (!res.ok) {
    throw new Error(`seed ${provider} -> ${res.status} ${text}`);
  }
  return {
    provider,
    skipped: false,
    id: data?.subscription?.id || null,
    key_label: data?.subscription?.key_label || null,
  };
}

async function main() {
  const args = parseArgs();
  const cfg = await loadConfig();
  const results = [];

  if (args.providers.has('codex')) {
    const codexAuth = await loadAuthJson(args.codexAuth, 'codex');
    results.push(await seedProvider(cfg, 'codex', codexAuth, args.force));
  }
  if (args.providers.has('kimi')) {
    const kimiAuth = await loadAuthJson(args.kimiAuth, 'kimi');
    results.push(await seedProvider(cfg, 'kimi', kimiAuth, args.force));
  }

  console.log(JSON.stringify({
    ok: true,
    agentId: cfg.agentId,
    results,
  }, null, 2));
}

main().catch((error) => die(error.stack || error.message || String(error)));
