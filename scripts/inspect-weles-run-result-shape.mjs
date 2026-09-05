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

const [runId] = process.argv.slice(2);
if (!runId) throw new Error('usage: node scripts/inspect-weles-run-result-shape.mjs <run-id>');

const env = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));
const supabaseUrl = env.SUPABASE_URL || env.NEXT_PUBLIC_SUPABASE_URL;
const serviceRole = env.SUPABASE_SERVICE_ROLE_KEY;
if (!supabaseUrl || !serviceRole) throw new Error('missing Weles Supabase service config');

const res = await fetch(`${supabaseUrl.replace(/\/+$/, '')}/rest/v1/account_action_logs?select=id,status,error,result,completed_at&id=eq.${encodeURIComponent(runId)}`, {
  headers: {
    apikey: serviceRole,
    Authorization: `Bearer ${serviceRole}`,
  },
});
const rows = await res.json();
if (!res.ok) throw new Error(`${res.status} ${JSON.stringify(rows).slice(0, 500)}`);
const row = rows[0];
if (!row) throw new Error(`run not found: ${runId}`);

function shape(value, depth = 0) {
  if (depth > 4) return typeof value;
  if (value === null) return 'null';
  if (Array.isArray(value)) return { type: 'array', length: value.length, first: value.length ? shape(value[0], depth + 1) : null };
  if (typeof value === 'object') {
    const out = {};
    for (const [key, child] of Object.entries(value)) {
      out[key] = shape(child, depth + 1);
    }
    return out;
  }
  if (typeof value === 'string') {
    return {
      type: 'string',
      length: value.length,
      looksJson: value.trim().startsWith('{'),
      hasClaudeAiOauth: value.includes('claudeAiOauth'),
      hasAccessToken: value.includes('accessToken'),
      prefix: value.slice(0, 80).replace(/[A-Za-z0-9_-]{24,}/g, '[REDACTED]'),
    };
  }
  return typeof value;
}

console.log(JSON.stringify({
  id: row.id,
  status: row.status,
  completed_at: row.completed_at,
  error: row.error,
  resultShape: shape(row.result),
}, null, 2));
