// Revoke active runtime subscriptions for one provider using the router API.
// Reads router config from Weles service_credentials metadata.

const WELLES_SUPABASE_URL = process.env.SUPABASE_URL;
const WELLES_SUPABASE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY;

function die(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs() {
  const args = { provider: '' };
  for (let i = 2; i < process.argv.length; i += 1) {
    const arg = process.argv[i];
    if (arg === '--provider') args.provider = process.argv[++i] || '';
    else throw new Error(`unknown arg: ${arg}`);
  }
  if (!args.provider) throw new Error('missing --provider');
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

async function activeSubscriptions(cfg) {
  const res = await fetch(`${cfg.routerUrl}/v1/subscriptions/${encodeURIComponent(cfg.agentId)}`);
  const text = await res.text();
  if (!res.ok) throw new Error(`read active subscriptions -> ${res.status} ${text}`);
  const data = JSON.parse(text);
  return Array.isArray(data.subscriptions) ? data.subscriptions : [];
}

async function revoke(cfg, row) {
  const body = JSON.stringify({
    user_id: row.donor_id,
    subscription_id: row.id,
  });
  const res = await fetch(`${cfg.routerUrl}/v1/subscriptions/${encodeURIComponent(cfg.agentId)}`, {
    method: 'DELETE',
    headers: { 'content-type': 'application/json' },
    body,
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`revoke ${row.provider}/${row.id} -> ${res.status} ${text}`);
  return { id: row.id, provider: row.provider, donor_id: row.donor_id };
}

async function main() {
  const args = parseArgs();
  const cfg = await loadConfig();
  const rows = (await activeSubscriptions(cfg)).filter((row) => row.provider === args.provider);
  const revoked = [];
  for (const row of rows) revoked.push(await revoke(cfg, row));
  console.log(JSON.stringify({ ok: true, agentId: cfg.agentId, provider: args.provider, revoked }, null, 2));
}

main().catch((error) => die(error.stack || error.message || String(error)));
