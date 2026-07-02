import { readFile } from 'node:fs/promises';

function parseEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(line.trim());
    if (!match) continue;
    let value = match[2].trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) value = value.slice(1, -1);
    out[match[1]] = value.replace(/\\n/g, '').trim();
  }
  return out;
}

const env = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));
const base = (env.NEXT_PUBLIC_BASE_URL || 'https://weles.wisent.com').replace(/\/+$/, '');
const token = env.WELES_DIAG_API_TOKEN || env.WELES_API_TOKEN;
if (!token) throw new Error('missing Weles API token');

const actions = process.argv.slice(2);
if (actions.length === 0) actions.push('claude_reauth');

for (const action of actions) {
  const listRes = await fetch(`${base}/api/v1/runs?action=${encodeURIComponent(action)}&limit=50`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  const listBody = await listRes.json();
  if (!listRes.ok) throw new Error(`${listRes.status} ${JSON.stringify(listBody)}`);
  const active = (listBody.rows || [])
    .filter((row) => row.status === 'queued' || row.status === 'running')
    .sort((a, b) => String(a.scheduled_at || '').localeCompare(String(b.scheduled_at || '')));
  const keep = active[0] ?? null;
  const cancel = active.slice(1);
  const cancelled = [];
  for (const row of cancel) {
    const res = await fetch(`${base}/api/v1/runs/${row.id}/cancel`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ reason: `duplicate ${action}; keeping ${keep?.id ?? 'none'}` }),
    });
    const body = await res.json().catch(() => ({}));
    cancelled.push({ id: row.id, status: res.status, ok: res.ok, error: body.error ?? null });
  }
  console.log(JSON.stringify({
    ok: true,
    action,
    kept: keep ? { id: keep.id, status: keep.status, scheduled_at: keep.scheduled_at } : null,
    cancelled,
  }, null, 2));
}
