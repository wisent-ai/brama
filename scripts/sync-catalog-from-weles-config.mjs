// Run sync-subscription-catalog.mjs with model-router target credentials
// loaded from Weles service_credentials metadata.

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

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
  for (const key of ['MR_SUPABASE_URL', 'MR_SUPABASE_SERVICE_ROLE_KEY']) {
    if (!meta[key]) throw new Error(`config missing ${key}`);
  }
  return meta;
}

function run(meta) {
  const child = spawn('node', ['scripts/sync-subscription-catalog.mjs', '--json'], {
    cwd: fileURLToPath(new URL('..', import.meta.url)),
    env: {
      ...process.env,
      TARGET_SUPABASE_URL: meta.MR_SUPABASE_URL,
      TARGET_SUPABASE_SERVICE_ROLE_KEY: meta.MR_SUPABASE_SERVICE_ROLE_KEY,
      WELES_SUPABASE_URL: WELLES_SUPABASE_URL,
      WELES_SUPABASE_SERVICE_ROLE_KEY: WELLES_SUPABASE_KEY,
    },
    stdio: ['ignore', 'inherit', 'inherit'],
  });
  child.on('exit', (code, signal) => {
    if (signal) die(`sync terminated by ${signal}`);
    process.exit(code ?? 1);
  });
}

loadConfig().then(run).catch((error) => die(error.stack || error.message || String(error)));
