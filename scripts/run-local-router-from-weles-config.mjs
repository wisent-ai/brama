// Start a local model-router server using Weles service_credentials metadata.
// This is for end-to-end local verification without printing secrets.

import { spawn } from 'node:child_process';

const WELLES_SUPABASE_URL = process.env.SUPABASE_URL;
const WELLES_SUPABASE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY;

function die(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs() {
  const out = { port: '18080' };
  for (let i = 2; i < process.argv.length; i += 1) {
    const arg = process.argv[i];
    if (arg === '--port') out.port = process.argv[++i] || '';
    else throw new Error(`unknown arg: ${arg}`);
  }
  if (!out.port) throw new Error('missing --port');
  return out;
}

async function loadMetadata() {
  if (!WELLES_SUPABASE_URL || !WELLES_SUPABASE_KEY) {
    die('Set SUPABASE_URL and SUPABASE_SERVICE_ROLE_KEY for Weles');
  }
  const headers = {
    apikey: WELLES_SUPABASE_KEY,
    Authorization: `Bearer ${WELLES_SUPABASE_KEY}`,
  };
  const res = await fetch(
    `${WELLES_SUPABASE_URL}/rest/v1/service_credentials?select=id,metadata`,
    { headers }
  );
  const text = await res.text();
  if (!res.ok) throw new Error(`read Weles config -> ${res.status} ${text}`);
  const rows = JSON.parse(text);
  const preferred = [
    'model-router-reauth-config',
    'weles-reauth-config',
    'codex-reauth-config',
    'claude-reauth-config',
    'weles-api-config',
  ];
  const env = {};
  for (const id of preferred) {
    const row = rows.find((r) => r.id === id);
    if (row?.metadata && typeof row.metadata === 'object') {
      for (const [key, value] of Object.entries(row.metadata)) {
        if (typeof value === 'string' && value) env[key] = value;
      }
    }
  }
  return env;
}

async function main() {
  const args = parseArgs();
  const metadata = await loadMetadata();
  const env = {
    ...process.env,
    ...metadata,
    SUPABASE_URL: metadata.SUPABASE_URL || metadata.MR_SUPABASE_URL || metadata.MODEL_ROUTER_SUPABASE_URL || process.env.SUPABASE_URL || process.env.NEXT_PUBLIC_SUPABASE_URL,
    SUPABASE_SERVICE_ROLE_KEY: metadata.SUPABASE_SERVICE_ROLE_KEY || metadata.MR_SUPABASE_SERVICE_ROLE_KEY || metadata.MODEL_ROUTER_SUPABASE_SERVICE_ROLE_KEY || process.env.SUPABASE_SERVICE_ROLE_KEY,
    WELES_SUPABASE_URL: process.env.SUPABASE_URL || process.env.NEXT_PUBLIC_SUPABASE_URL || metadata.WELES_SUPABASE_URL,
    WELES_SUPABASE_SERVICE_ROLE_KEY: process.env.SUPABASE_SERVICE_ROLE_KEY || metadata.WELES_SUPABASE_SERVICE_ROLE_KEY,
    AGENT_AUTH_SECRET: metadata.WISENT_APP_AGENT_AUTH_SECRET || process.env.AGENT_AUTH_SECRET,
  };
  const child = spawn('cargo', ['run', '--', 'serve', '--port', args.port], {
    cwd: process.cwd(),
    env,
    stdio: 'inherit',
  });
  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    process.exit(code ?? 1);
  });
}

main().catch((error) => die(error.stack || error.message || String(error)));
