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

const contentEnv = parseEnv(await readFile('../content-platform/.env.local', 'utf8'));
const webEnv = parseEnv(await readFile('../backends/weles-web/.env.local', 'utf8'));

const baseUrl = contentEnv.WELES_SUPABASE_URL;
const serviceRole = contentEnv.WELES_SUPABASE_SERVICE_ROLE_KEY;
const welesApiUrl = webEnv.NEXT_PUBLIC_BASE_URL || 'https://weles.wisent.com';
const token = webEnv.WELES_DIAG_API_TOKEN;
if (!baseUrl || !serviceRole || !token) throw new Error('missing required Weles config');

const rest = `${baseUrl.replace(/\/+$/, '')}/rest/v1/service_credentials`;
const headers = {
  apikey: serviceRole,
  Authorization: `Bearer ${serviceRole}`,
  'content-type': 'application/json',
};
const id = 'model-router-reauth-config';
const metadata = {
  WELES_API_URL: welesApiUrl,
  WELES_API_TOKEN: token,
  WELES_REAUTH_MODE: 'runs-api',
  updated_at: new Date().toISOString(),
  updated_by: 'model-router-repair',
};

const existing = await fetch(`${rest}?id=eq.${encodeURIComponent(id)}&select=id`, { headers });
const existingText = await existing.text();
if (!existing.ok) throw new Error(`check existing -> ${existing.status} ${existingText}`);
const rows = JSON.parse(existingText);
const method = rows.length ? 'PATCH' : 'POST';
const url = rows.length ? `${rest}?id=eq.${encodeURIComponent(id)}` : rest;
const body = rows.length ? { metadata } : {
  id,
  category: 'auth',
  display_name: 'Model-router Weles reauth API',
  dashboard_url: welesApiUrl,
  notes: 'Runtime config used by model-router to queue provider reauth runs through Weles API.',
  metadata,
};
const res = await fetch(url, {
  method,
  headers: {
    ...headers,
    Prefer: 'return=representation',
  },
  body: JSON.stringify(body),
});
const text = await res.text();
if (!res.ok) throw new Error(`upsert -> ${res.status} ${text}`);
console.log(JSON.stringify({
  ok: true,
  id,
  method,
  metadata: {
    WELES_API_URL: metadata.WELES_API_URL,
    WELES_API_TOKEN: '<redacted>',
    WELES_REAUTH_MODE: metadata.WELES_REAUTH_MODE,
  },
}, null, 2));
