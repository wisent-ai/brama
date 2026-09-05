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

const [runId, status = 'cancelled', ...reasonParts] = process.argv.slice(2);
if (!runId) throw new Error('usage: node scripts/mark-weles-run-terminal.mjs <run-id> [status] [reason]');
if (!['cancelled', 'failed', 'completed'].includes(status)) throw new Error(`unsupported status ${status}`);

const reason = reasonParts.join(' ') || `marked ${status} by model-router maintenance`;
const env = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));
const supabaseUrl = env.SUPABASE_URL || env.NEXT_PUBLIC_SUPABASE_URL;
const serviceRole = env.SUPABASE_SERVICE_ROLE_KEY;
if (!supabaseUrl || !serviceRole) throw new Error('missing Weles Supabase service config');

const patch = {
  status,
  completed_at: new Date().toISOString(),
  error: reason,
  cancel_requested: status === 'cancelled' ? true : undefined,
};
for (const key of Object.keys(patch)) {
  if (patch[key] === undefined) delete patch[key];
}

const res = await fetch(`${supabaseUrl.replace(/\/+$/, '')}/rest/v1/account_action_logs?id=eq.${encodeURIComponent(runId)}`, {
  method: 'PATCH',
  headers: {
    apikey: serviceRole,
    Authorization: `Bearer ${serviceRole}`,
    'Content-Type': 'application/json',
    Prefer: 'return=representation',
  },
  body: JSON.stringify(patch),
});
const body = await res.json().catch(() => ({}));
console.log(JSON.stringify({ ok: res.ok, status: res.status, id: runId, rows: Array.isArray(body) ? body.length : null }, null, 2));
if (!res.ok) process.exit(1);
