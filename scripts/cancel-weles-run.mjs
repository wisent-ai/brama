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

const runId = process.argv[2];
if (!runId) throw new Error('usage: node scripts/cancel-weles-run.mjs <run-id> [reason]');

const reason = process.argv.slice(3).join(' ') || 'manual cancellation';
const env = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));
const base = (env.NEXT_PUBLIC_BASE_URL || 'https://weles.wisent.com').replace(/\/+$/, '');
const token = env.WELES_DIAG_API_TOKEN || env.WELES_API_TOKEN;
if (!token) throw new Error('missing Weles API token');

const res = await fetch(`${base}/api/v1/runs/${encodeURIComponent(runId)}/cancel`, {
  method: 'POST',
  headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
  body: JSON.stringify({ reason }),
});
const body = await res.json().catch(() => ({}));
console.log(JSON.stringify({ ok: res.ok, status: res.status, id: runId, body }, null, 2));
if (!res.ok) process.exit(1);
