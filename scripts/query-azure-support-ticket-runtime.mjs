import { readFile } from 'node:fs/promises';

const ZERO = Number('0');
const ONE = Number('1');
const ENV_VALUE_INDEX = Number('2');
const TICKET_ID_INDEX = Number('2');
const RESOURCE_NAME_INDEX = Number('3');
const POLL_INTERVAL_MS = Number('5000');
const RUN_TIMEOUT_MS = Number('900000');

function parseEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z_][A-Za-z\d_]*)\s*=\s*(.*)$/.exec(line.trim());
    if (!match) continue;
    let value = match.at(ENV_VALUE_INDEX).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(ONE, -ONE);
    }
    out[match.at(ONE)] = value.replace(/\\n/g, '').trim();
  }
  return out;
}

function terminal(status) {
  return new Set(['completed', 'failed', 'cancelled', 'rejected', 'approved', 'pending_review']).has(status);
}

async function rest(origin, token, path, init = {}) {
  const response = await fetch(`${origin}/rest/v1/${path}`, {
    ...init,
    headers: {
      apikey: token,
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
      ...(init.headers ?? {}),
    },
    signal: AbortSignal.timeout(POLL_INTERVAL_MS),
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(`Weles runtime API failed (${response.status}): ${JSON.stringify(body)}`);
  return body;
}

const ticketId = process.argv.at(TICKET_ID_INDEX);
if (!ticketId) {
  throw new Error('usage: node scripts/query-azure-support-ticket-runtime.mjs <ticket-id> [ticket-resource-name]');
}
const ticketResourceName = process.argv.at(RESOURCE_NAME_INDEX) ?? '';
const envUrl = new URL('../../echo/.env.local', import.meta.url);
const env = parseEnv(await readFile(envUrl, 'utf8'));
const origin = (env.WELES_SUPABASE_URL || '').replace(/\/+$/, '');
const token = env.WELES_SUPABASE_SERVICE_ROLE_KEY;
if (!origin || !token) throw new Error('missing Weles runtime API credentials');

const timeWindow = new Date().toISOString().slice(ZERO, Number('16')).replace(/[-:T]/g, '');
const requestId = `azure-support-status-${ticketId}-${timeWindow}`;
const objective = [
  'Use only an already-authenticated Microsoft Azure browser session.',
  'Read only. Do not sign in, enter an email, enter a password, trigger a passkey, or use account recovery.',
  `Open Azure Support and inspect support ticket ${ticketId}${ticketResourceName ? ` with resource name ${ticketResourceName}` : ''}.`,
  'Return the current ticket status, assigned support engineer, latest Microsoft message with timestamp, SLA or response deadline, whether the two UnusualActivity deny assignments remain active, and the exact next action required.',
  'If no authenticated session exists, return authentication_required without attempting authentication.',
  'Do not create, update, reply to, close, or otherwise mutate any ticket, account, subscription, credential, or resource.',
].join(' ');

const query = new URLSearchParams({
  action: 'eq.generic_browser_task',
  'params->>request_id': `eq.${requestId}`,
  select: 'id,status,params,result,error,scheduled_at,started_at,completed_at,claimed_at,claimed_by',
  limit: String(ONE),
});
let rows = await rest(origin, token, `account_action_logs?${query}`);
if (!Array.isArray(rows) || rows.length === ZERO) {
  rows = await rest(origin, token, 'account_action_logs?select=id,status,params,result,error,scheduled_at,started_at,completed_at,claimed_at,claimed_by', {
    method: 'POST',
    headers: { Prefer: 'return=representation' },
    body: JSON.stringify({
      account_id: null,
      action: 'generic_browser_task',
      platform: 'generic',
      status: 'queued',
      scheduled_at: new Date().toISOString(),
      params: {
        request_id: requestId,
        url: 'https://portal.azure.com/#view/Microsoft_Azure_Support/HelpAndSupportBlade/~/managesupportrequest',
        objective,
        constraints: ['read_only', 'no_authentication', 'no_mutation'],
        flow_name: 'azure_support_ticket_status',
        session_label: 'azure-support',
        proxy: 'none',
        headless: true,
      },
    }),
  });
}
const runId = rows.at(ZERO)?.id;
if (!runId) throw new Error(`Weles runtime API did not return a run id: ${JSON.stringify(rows)}`);
console.error(JSON.stringify({ queued: true, run_id: runId }));

const deadline = Date.now() + RUN_TIMEOUT_MS;
let run = rows.at(ZERO);
while (!terminal(run.status) && Date.now() < deadline) {
  await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
  const current = await rest(origin, token, `account_action_logs?id=eq.${encodeURIComponent(runId)}&select=id,status,params,result,error,scheduled_at,started_at,completed_at,claimed_at,claimed_by&limit=1`);
  run = current.at(ZERO) ?? run;
}

console.log(JSON.stringify({
  run_id: run.id,
  status: run.status,
  scheduled_at: run.scheduled_at,
  started_at: run.started_at,
  completed_at: run.completed_at,
  claimed_at: run.claimed_at,
  claimed_by: run.claimed_by,
  error: run.error,
  result: run.result,
}, null, ENV_VALUE_INDEX));
if (!terminal(run.status)) process.exit(Number('2'));
if (!new Set(['completed', 'approved', 'pending_review']).has(run.status)) process.exit(ONE);
