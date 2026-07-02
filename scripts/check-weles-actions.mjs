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
const base = env.NEXT_PUBLIC_BASE_URL || 'https://weles.wisent.com';
const token = env.WELES_DIAG_API_TOKEN;
if (!token) throw new Error('missing Weles API token');
const res = await fetch(`${base.replace(/\/+$/, '')}/api/v1/actions`, {
  headers: { Authorization: `Bearer ${token}` },
});
const body = await res.json();
if (!res.ok) throw new Error(`${res.status} ${JSON.stringify(body)}`);
console.log(JSON.stringify({
  ok: true,
  allow_custom_actions: body.allow_custom_actions,
  actions: (body.actions || []).map((action) => action.action).filter((name) => /reauth|claude|codex|kimi|generic/.test(name)),
}, null, 2));

const trajectoriesRes = await fetch(`${base.replace(/\/+$/, '')}/api/v1/trajectories?limit=500`, {
  headers: { Authorization: `Bearer ${token}` },
});
const trajectoriesBody = await trajectoriesRes.json();
if (!trajectoriesRes.ok) throw new Error(`${trajectoriesRes.status} ${JSON.stringify(trajectoriesBody)}`);
console.log(JSON.stringify({
  ok: true,
  trajectories: (trajectoriesBody.rows || trajectoriesBody.trajectories || [])
    .filter((row) => JSON.stringify(row).toLowerCase().match(/reauth|claude|codex|kimi/))
    .map((row) => ({
      id: row.id,
      name: row.name,
      action: row.action,
      site: row.site,
      status: row.status,
    })),
}, null, 2));

for (const action of ['claude_reauth', 'codex_reauth', 'kimi_reauth']) {
  const runsRes = await fetch(`${base.replace(/\/+$/, '')}/api/v1/runs?action=${action}&limit=10`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  const runsBody = await runsRes.json();
  if (!runsRes.ok) throw new Error(`${runsRes.status} ${JSON.stringify(runsBody)}`);
  console.log(JSON.stringify({
    ok: true,
    action,
    runs: runsBody.runs,
    rows: (runsBody.rows || []).map((row) => ({
      id: row.id,
      action: row.action,
      status: row.status,
      scheduled_at: row.scheduled_at,
      started_at: row.started_at,
      completed_at: row.completed_at,
      claimed_by: row.claimed_by,
      error: row.error,
      result_keys: row.result && typeof row.result === 'object' ? Object.keys(row.result) : [],
    })),
  }, null, 2));
}
