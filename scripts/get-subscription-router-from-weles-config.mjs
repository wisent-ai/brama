// Fetch the model-router subscription-router snapshot using agent config
// stored in Weles service_credentials metadata.

import crypto from 'node:crypto';

const WELLES_SUPABASE_URL = process.env.SUPABASE_URL;
const WELLES_SUPABASE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY;

function die(message) {
  console.error(message);
  process.exit(1);
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
  for (const key of ['MODEL_ROUTER_URL', 'WISENT_APP_AGENT_ID', 'WISENT_APP_AGENT_AUTH_SECRET']) {
    if (!meta[key]) throw new Error(`config missing ${key}`);
  }
  return {
    routerUrl: String(meta.MODEL_ROUTER_URL).replace(/\/+$/, ''),
    agentId: String(meta.WISENT_APP_AGENT_ID),
    hmacSecret: String(meta.WISENT_APP_AGENT_AUTH_SECRET),
  };
}

function signGet(cfg) {
  const ts = String(Math.floor(Date.now() / 1000));
  const bodyHash = '';
  const msg = `${cfg.agentId}:${ts}:${bodyHash}`;
  const sig = crypto.createHmac('sha256', cfg.hmacSecret).update(msg).digest('hex');
  return {
    'x-agent-id': cfg.agentId,
    'x-agent-timestamp': ts,
    'x-agent-signature': sig,
  };
}

async function main() {
  const cfg = await loadConfig();
  const res = await fetch(`${cfg.routerUrl}/v1/subscription-router/${encodeURIComponent(cfg.agentId)}`, {
    headers: signGet(cfg),
  });
  const text = await res.text();
  let data;
  try { data = JSON.parse(text); } catch { data = { raw: text }; }
  if (!res.ok) {
    console.log(JSON.stringify({ ok: false, status: res.status, data }, null, 2));
    process.exit(1);
  }
  const rows = Array.isArray(data.subscriptions) ? data.subscriptions : [];
  const read = (row, snake, camel) => row[camel] ?? row[snake] ?? null;
  console.log(JSON.stringify({
    ok: true,
    status: res.status,
    authenticated: data.authenticated,
    summary: data.summary,
    checkSummary: data.checkSummary,
    rows: rows.map((row) => ({
      kind: row.kind || row.sourceKind,
      service: row.service,
      provider: row.provider,
      account_identifier: read(row, 'account_identifier', 'accountIdentifier'),
      status: row.status,
      plan: row.plan,
      monthly_cost_usd: read(row, 'monthly_cost_usd', 'monthlyCostUSD'),
      period_cost_usd: read(row, 'period_cost_usd', 'periodCostUSD'),
      expires_at: read(row, 'expires_at', 'expiresAt'),
      cost_status: row.costStatus || row.cost_status || null,
      source: row.source,
    })),
  }, null, 2));
}

main().catch((error) => die(error.stack || error.message || String(error)));
