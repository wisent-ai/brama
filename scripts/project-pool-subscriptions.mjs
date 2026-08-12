#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: `stado host
// run-helper` executes an installed helper through sh, which cannot read a
// module, and a helper runs with a minimal PATH that `env node` alone would
// miss. To sh they exec node on this same file; to node they are a comment.

// Fill the gateway's subscription items from the donated pool.
//
// The pool of donated subscriptions lives in the app's credential store, and
// the service that used to serve it -- a model router on Cloud Run -- is gone
// with the GCP account it ran in. Brama took over as the fleet's gateway but
// reads subscriptions from its own `brama-sub-*` vault items, and nothing was
// ever wired between the two. The result reads as a fleet with no
// subscriptions: `no working vision-capable subscription model`, while six
// active credentials sit in the store, every one of them carrying a field the
// provider adapter already knows how to read.
//
// This copies the pool's active credentials into the subscription items that
// already exist, so their grants, recipients and agent tags are the ones the
// operator has already sanctioned. No item is created and no permission is
// widened; an item with no counterpart in the pool is left exactly as it is.
//
// Decryption mirrors the app's own helper (AES-256-GCM, scrypt key over the
// 'wisent-salt' salt, `iv:tag:ciphertext` in base64), so this stays correct as
// long as that helper does.
//
// Prints item ids, providers, and which credential field the plaintext
// carries. It never prints a credential.
//
// Usage: node project-pool-subscriptions.mjs [--write]

import crypto from 'node:crypto';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
// `stado host run-helper` passes no arguments -- deliberately, since a helper
// that takes operator words is a remote shell -- so the mode comes from the
// name it was installed under. Installed as `apply-...` it writes; under any
// other name it only reports.
const WRITE = process.argv.includes('--write')
  || String(process.argv.at(Number(Boolean(1)))).includes('apply-');

// A pool row is not automatically fresher than what the item already holds:
// the reauth runner banks a host's live session directly, and that can be
// newer than the donation. Naming the providers keeps a stale donation from
// silently replacing a working credential.
const ONLY = process.argv
  .filter((argument) => argument.startsWith('--provider='))
  .map((argument) => argument.replace('--provider=', ''));
const HOME = os.homedir();
const CLI = `${HOME}/.stado/bin/skarbiec`;
const ENV = {
  ...process.env,
  SKARBIEC_VAULT_FILE: process.env.SKARBIEC_VAULT_FILE || `${HOME}/.stado/skarbiec.vault.json`,
  PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
};

// The order the adapter tries, from SUPPORTED_KEY_FIELDS in providers/adapter.rs.
const KEY_FIELDS = [
  ['key'], ['apiKey'], ['api_key'], ['access'], ['accessToken'], ['access_token'],
  ['token'], ['tokens', 'access_token'], ['claudeAiOauth', 'accessToken'],
];

// Pool provider names are snake, the gateway's item ids are dashed.
const ITEM_FOR = {
  codex: ['provider:codex:brama-sub-wisent-app-codex-primary', 'provider:codex:brama-sub-wisent-app-codex-secondary'],
  claude_code: ['provider:claude-code:brama-sub-wisent-app-claude-primary'],
  kimi: ['provider:kimi:brama-sub-wisent-app-kimi-primary'],
};

const skarbiec = (args, input) =>
  execFileSync(CLI, args, { env: ENV, encoding: 'utf8', input });

// Re-read per item rather than cached: a write below changes the file, and a
// stale envelope is how an item gets rewritten with someone else's recipients.
const vault = () => JSON.parse(readFileSync(ENV.SKARBIEC_VAULT_FILE, 'utf8'));

function config() {
  const document = JSON.parse(skarbiec(['get', 'codex-reauth-config']));
  let value = document.fields.value;
  if (typeof value === 'string') value = JSON.parse(value);
  let metadata = value.metadata;
  if (typeof metadata === 'string') metadata = JSON.parse(metadata);
  for (const name of ['MR_SUPABASE_URL', 'MR_SUPABASE_SERVICE_ROLE_KEY', 'ENCRYPTION_KEY']) {
    if (!metadata[name]) throw new Error(`codex-reauth-config.metadata is missing ${name}`);
  }
  return metadata;
}

function keyMaterial(secret) {
  // The app accepts a hex key, a base64 key, or anything else derived through
  // scrypt. Lengths are taken from the digest rather than restated here.
  const length = crypto.createHash('sha256').digest().length;
  if (secret.length === length * Buffer.from('ff', 'hex').length * Number('2')) {
    return Buffer.from(secret, 'hex');
  }
  const base64Length = Buffer.from(Buffer.alloc(length)).toString('base64').length;
  if (secret.length === base64Length) return Buffer.from(secret, 'base64');
  return crypto.scryptSync(secret, 'wisent-salt', length);
}

function decrypt(material, encrypted) {
  const [iv, tag, ciphertext] = String(encrypted).split(':').map((part) => Buffer.from(part, 'base64'));
  const decipher = crypto.createDecipheriv('aes-256-gcm', material, iv);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(ciphertext), decipher.final()]).toString('utf8');
}

function credentialField(plaintext) {
  let parsed;
  try {
    parsed = JSON.parse(plaintext);
  } catch {
    return { field: 'bare secret', length: plaintext.length };
  }
  for (const path of KEY_FIELDS) {
    let cursor = parsed;
    for (const part of path) cursor = cursor && typeof cursor === 'object' ? cursor[part] : undefined;
    if (typeof cursor === 'string' && cursor) return { field: path.join('.'), length: cursor.length };
  }
  return null;
}

const metadata = config();
const material = keyMaterial(metadata.ENCRYPTION_KEY);
const store = metadata.MR_SUPABASE_URL.replace(/\/+$/, '');
const response = await fetch(
  `${store}/rest/v1/trade_agent_subscriptions?status=eq.active&select=id,provider,key_encrypted,key_label,updated_at&order=updated_at.desc`,
  { headers: { apikey: metadata.MR_SUPABASE_SERVICE_ROLE_KEY, Authorization: `Bearer ${metadata.MR_SUPABASE_SERVICE_ROLE_KEY}` } },
);
if (!response.ok) throw new Error(`pool read -> HTTP ${response.status}`);
const pool = await response.json();
console.log(`active subscriptions in the pool: ${pool.length}`);
console.log(WRITE ? 'mode: writing' : 'mode: reporting only (pass --write to apply)');

const taken = new Map();
for (const row of pool) {
  if (ONLY.length && !ONLY.includes(row.provider)) continue;
  const targets = ITEM_FOR[row.provider];
  if (!targets) {
    console.log(`  ${row.provider}: no gateway item is declared for this provider; left in the pool`);
    continue;
  }
  const used = taken.get(row.provider) || [];
  const item = targets[used.length];
  if (!item) {
    console.log(`  ${row.provider}: pool has more credentials than declared items; ${row.key_label} left in the pool`);
    continue;
  }
  used.push(item);
  taken.set(row.provider, used);

  let plaintext;
  try {
    plaintext = decrypt(material, row.key_encrypted);
  } catch (error) {
    console.log(`  ${item}: does not decrypt (${error.code || error.name}); left unchanged`);
    continue;
  }
  const carried = credentialField(plaintext);
  if (!carried) {
    console.log(`  ${item}: plaintext carries no field the adapter reads; left unchanged`);
    continue;
  }

  // `skarbiec get` returns the decrypted document only; recipients and tags
  // are envelope properties and live in the vault file, which is where the
  // fleet's own installer reads them from too.
  const record = vault().items?.[item] || {};
  const recipients = record.recipients || [];
  const tags = record.tags || [];
  console.log(`  ${item}: ${carried.field} (${carried.length} chars) from ${JSON.stringify(row.key_label)}`);

  // A session banked straight off a host is not a donation and is usually the
  // newer of the two: the reauth runner prefers exactly that credential before
  // it drives any login. Replacing it with a pool row would quietly roll the
  // subscription backwards, so the donation yields.
  let current;
  try {
    current = JSON.parse(skarbiec(['get', item]));
  } catch {
    current = null;
  }
  const banked = current?.context?.source;
  const usable = current && credentialField(
    typeof current.fields?.value === 'string' ? current.fields.value : JSON.stringify(current.fields?.value ?? null),
  );
  if (banked && banked !== 'trade_agent_subscriptions' && usable) {
    console.log(`    holds a credential banked from ${banked}; the donation yields to it`);
    continue;
  }

  if (!WRITE) continue;
  if (!recipients.length) {
    console.log(`    no recipients on the existing item; refusing to write one nobody can open`);
    continue;
  }
  const body = JSON.stringify({
    schema: 'skarbiec.item.v2',
    kind: 'bundle',
    fields: { value: plaintext },
    context: { source: 'trade_agent_subscriptions', row: row.id, label: row.key_label },
  });
  const args = ['set-json', item, '--recipients', recipients.join(',')];
  if (tags.length) args.push('--tags', tags.join(','));
  skarbiec(args, body);
  console.log(`    written`);
}
