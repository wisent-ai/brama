import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

function parseEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(line.trim());
    if (!match) continue;
    let value = match[2].trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    value = value.replace(/\\n/g, '').trim();
    out[match[1]] = value;
  }
  return out;
}

function redacted(key, value) {
  if (/KEY|SECRET|TOKEN|PASSWORD|AUTH/i.test(key)) return '<redacted>';
  return value;
}

function interestingKeys(metadata) {
  return Object.entries(metadata || {})
    .filter(([key]) => /WELES|WELLES|MAC|MINI|REAUTH|ROUTER|API_URL|BASE_URL/i.test(key))
    .map(([key, value]) => [key, typeof value === 'string' ? redacted(key, value) : value]);
}

const envPath = resolve('../content-platform/.env.local');
const env = parseEnv(await readFile(envPath, 'utf8'));
const baseUrl = env.WELES_SUPABASE_URL;
const serviceRole = env.WELES_SUPABASE_SERVICE_ROLE_KEY;
if (!baseUrl || !serviceRole) throw new Error('missing Weles Supabase config in content-platform .env.local');

const res = await fetch(`${baseUrl.replace(/\/+$/, '')}/rest/v1/service_credentials?select=*&limit=80`, {
  headers: {
    apikey: serviceRole,
    Authorization: `Bearer ${serviceRole}`,
  },
});
const text = await res.text();
if (!res.ok) throw new Error(`read service_credentials -> ${res.status} ${text}`);
const rows = JSON.parse(text);
console.log(JSON.stringify({
  ok: true,
  rows: rows.map((row) => ({
    id: row.id,
    topLevelKeys: Object.keys(row).sort(),
    category: row.category ?? null,
    service: row.service ?? null,
    name: row.name ?? null,
    keys: Object.keys(row.metadata || {}).sort(),
    interesting: Object.fromEntries(interestingKeys(row.metadata)),
  })),
}, null, 2));
