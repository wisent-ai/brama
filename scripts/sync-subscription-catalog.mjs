#!/usr/bin/env node
// Sync subscription catalog metadata into model-router.
//
// This is intentionally metadata-only. It does not read or move runtime tokens,
// passwords, or OAuth blobs. Runtime credentials stay in trade_agent_subscriptions;
// this catalog holds billing/status metadata used by subscription-router.

import { existsSync, readFileSync } from 'node:fs';

function parseArgs(argv) {
  const out = { dryRun: false, json: false, welesEnv: '' };
  for (let i = 2; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--dry-run') out.dryRun = true;
    else if (arg === '--json') out.json = true;
    else if (arg === '--weles-env') out.welesEnv = argv[++i] || '';
    else throw new Error(`unknown arg: ${arg}`);
  }
  return out;
}

function parseDotenv(path) {
  if (!path || !existsSync(path)) return {};
  const text = readFileSync(path, 'utf8');
  const env = {};
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const idx = line.indexOf('=');
    if (idx <= 0) continue;
    const key = line.slice(0, idx).trim();
    let value = line.slice(idx + 1).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    env[key] = value;
  }
  return env;
}

function required(value, name) {
  if (!value) throw new Error(`missing ${name}`);
  return value;
}

function headers(key, prefer) {
  const out = {
    apikey: key,
    Authorization: `Bearer ${key}`,
    'Content-Type': 'application/json',
  };
  if (prefer) out.Prefer = prefer;
  return out;
}

async function restJSON({ baseURL, key, path, method = 'GET', body, prefer }) {
  const res = await fetch(`${baseURL.replace(/\/+$/, '')}/rest/v1/${path}`, {
    method,
    headers: headers(key, prefer),
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${method} ${path} failed: ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

function asNumberOrNull(value) {
  if (value === null || value === undefined || value === '') return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

function transformWelesRow(row, collectedAt) {
  return {
    agent_id: null,
    source: 'weles',
    provider: row.provider || 'unknown',
    service: row.service_name || row.service || 'unknown',
    account_identifier: row.account_identifier || null,
    status: row.status || 'unknown',
    plan: row.plan || null,
    monthly_cost_usd: asNumberOrNull(row.monthly_cost_usd),
    period_cost_usd: asNumberOrNull(row.monthly_cost_usd),
    expires_at: row.expires_at || null,
    last_verified_at: row.last_verified_at || collectedAt,
    metadata: {
      collector: 'weles_service_subscriptions',
      collectedAt,
      source: 'weles.service_subscriptions',
      welesId: row.id || null,
      metadata: row.metadata || {},
    },
  };
}

function countBy(rows, field) {
  const counts = {};
  for (const row of rows) {
    const key = row[field] || 'unknown';
    counts[key] = (counts[key] || 0) + 1;
  }
  return counts;
}

async function main() {
  const args = parseArgs(process.argv);
  const welesEnv = parseDotenv(args.welesEnv || process.env.WELES_ENV_PATH || '');

  const targetURL = required(
    process.env.TARGET_SUPABASE_URL
      || process.env.MODEL_ROUTER_SUPABASE_URL
      || process.env.SUPABASE_URL,
    'TARGET_SUPABASE_URL or SUPABASE_URL',
  );
  const targetKey = required(
    process.env.TARGET_SUPABASE_SERVICE_ROLE_KEY
      || process.env.MODEL_ROUTER_SUPABASE_SERVICE_ROLE_KEY
      || process.env.SUPABASE_SERVICE_ROLE_KEY,
    'TARGET_SUPABASE_SERVICE_ROLE_KEY or SUPABASE_SERVICE_ROLE_KEY',
  );
  const sourceURL = required(
    process.env.WELES_SUPABASE_URL
      || welesEnv.SUPABASE_URL,
    'WELES_SUPABASE_URL or --weles-env with SUPABASE_URL',
  );
  const sourceKey = required(
    process.env.WELES_SUPABASE_SERVICE_ROLE_KEY
      || welesEnv.SUPABASE_SERVICE_ROLE_KEY,
    'WELES_SUPABASE_SERVICE_ROLE_KEY or --weles-env with SUPABASE_SERVICE_ROLE_KEY',
  );

  const query = new URLSearchParams({
    select: 'id,service_name,provider,account_identifier,status,plan,monthly_cost_usd,expires_at,last_verified_at,metadata,created_at,updated_at',
    order: 'service_name.asc',
  });
  const sourceRows = await restJSON({
    baseURL: sourceURL,
    key: sourceKey,
    path: `service_subscriptions?${query.toString()}`,
  });
  const collectedAt = new Date().toISOString();
  const targetRows = sourceRows.map((row) => transformWelesRow(row, collectedAt));

  if (!args.dryRun) {
    await restJSON({
      baseURL: targetURL,
      key: targetKey,
      path: 'subscription_router_entries?source=eq.weles',
      method: 'DELETE',
    });
    if (targetRows.length) {
      await restJSON({
        baseURL: targetURL,
        key: targetKey,
        path: 'subscription_router_entries',
        method: 'POST',
        body: targetRows,
        prefer: 'return=minimal',
      });
    }
  }

  const result = {
    ok: true,
    dryRun: args.dryRun,
    source: 'weles.service_subscriptions',
    target: 'model-router.subscription_router_entries',
    collectedAt,
    rows: targetRows.length,
    byStatus: countBy(targetRows, 'status'),
    byProvider: countBy(targetRows, 'provider'),
  };
  if (args.json) {
    console.log(JSON.stringify(result, null, 2));
  } else {
    console.log(`synced ${result.rows} subscription catalog row(s) from ${result.source}`);
    console.log(`status=${JSON.stringify(result.byStatus)} provider=${JSON.stringify(result.byProvider)}`);
  }
}

main().catch((error) => {
  console.error(`subscription catalog sync failed: ${error.message}`);
  process.exit(1);
});
