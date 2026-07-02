// Send one signed /v1/chat/completions request using agent config
// stored in Weles service_credentials metadata.

import crypto from 'node:crypto';

const WELLES_SUPABASE_URL = process.env.SUPABASE_URL;
const WELLES_SUPABASE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY;

function die(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs() {
  const out = {
    model: '',
    omitModel: false,
    prompt: 'Reply with exactly OK.',
  };
  for (let i = 2; i < process.argv.length; i += 1) {
    const arg = process.argv[i];
    if (arg === '--model') out.model = process.argv[++i] || '';
    else if (arg === '--omit-model') out.omitModel = true;
    else if (arg === '--prompt') out.prompt = process.argv[++i] || '';
    else throw new Error(`unknown arg: ${arg}`);
  }
  if (!out.omitModel && !out.model) throw new Error('missing --model');
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
  for (const key of ['MODEL_ROUTER_URL', 'WISENT_APP_AGENT_ID', 'WISENT_APP_AGENT_AUTH_SECRET']) {
    if (!meta[key]) throw new Error(`config missing ${key}`);
  }
  return {
    routerUrl: String(process.env.MODEL_ROUTER_URL || meta.MODEL_ROUTER_URL).replace(/\/+$/, ''),
    agentId: String(meta.WISENT_APP_AGENT_ID),
    hmacSecret: String(meta.WISENT_APP_AGENT_AUTH_SECRET),
  };
}

function sign(cfg, body) {
  const ts = String(Math.floor(Date.now() / 1000));
  const bodyHash = crypto.createHash('sha256').update(body).digest('hex');
  const msg = `${cfg.agentId}:${ts}:${bodyHash}`;
  const sig = crypto.createHmac('sha256', cfg.hmacSecret).update(msg).digest('hex');
  return {
    'content-type': 'application/json',
    'x-agent-id': cfg.agentId,
    'x-agent-timestamp': ts,
    'x-agent-signature': sig,
  };
}

async function main() {
  const args = parseArgs();
  const cfg = await loadConfig();
  const request = {
    messages: [{ role: 'user', content: args.prompt }],
    max_tokens: 8,
    temperature: 0,
  };
  if (!args.omitModel) request.model = args.model;
  const body = JSON.stringify(request);
  const res = await fetch(`${cfg.routerUrl}/v1/chat/completions`, {
    method: 'POST',
    headers: sign(cfg, body),
    body,
  });
  const text = await res.text();
  let data;
  try { data = JSON.parse(text); } catch { data = { raw: text }; }
  const choice = data?.choices?.[0]?.message?.content || data?.content || '';
  console.log(JSON.stringify({
    ok: res.ok,
    status: res.status,
    requested_model: args.omitModel ? null : args.model,
    response_model: data?.model || null,
    content: String(choice).slice(0, 300),
    error: data?.error?.message || data?.error || null,
  }, null, 2));
  if (!res.ok) process.exit(1);
}

main().catch((error) => die(error.stack || error.message || String(error)));
