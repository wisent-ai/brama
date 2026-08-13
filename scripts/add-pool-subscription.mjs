#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Create a vault item for a paid subscription the fleet holds but never banked.
//
// The gateway discovers a subscription from any vault item named
// `provider:<provider>:brama-sub-<agent>-<name>`, so an account with no such
// item is invisible no matter how healthy its credential is. Two donated Claude
// credentials are in exactly that state, one of them a whole paid account, and
// the projection that fills the four hand-named items reported them as left in
// the pool rather than creating anywhere for them to land.
//
// This creates that item: the credential is decrypted from the donation pool,
// the recipients and agent tags are copied from the sibling item of the same
// provider so nothing is widened, and the id says which account it is.
//
// Reads the target account from POOL_ACCOUNT (a substring of the donation
// label) and the item suffix from ITEM_SUFFIX. Prints names only.

import crypto from 'node:crypto';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const HOME = os.homedir();
const CLI = `${HOME}/.stado/bin/skarbiec`;
const ENV = {
  ...process.env,
  SKARBIEC_VAULT_FILE: process.env.SKARBIEC_VAULT_FILE || `${HOME}/.stado/skarbiec.vault.json`,
  PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
};
const ACCOUNT = process.env.POOL_ACCOUNT || 'Claude_controlyourai';
const SUFFIX = process.env.ITEM_SUFFIX || 'claude-controlyourai';
const AGENT = process.env.POOL_AGENT || 'wisent-app';
// An installed helper is run with no arguments -- deliberately, since a helper
// that takes operator words is a remote shell -- so the mode comes from the
// name it was installed under: `add-...` creates the item, anything else only
// reports what it would create.
const [, invokedAs] = process.argv;
const WRITE = process.argv.includes('--write') || /(^|\/)add-/.test(String(invokedAs));

const skarbiec = (args, input) => execFileSync(CLI, args, { env: ENV, encoding: 'utf8', input });
const vault = () => JSON.parse(readFileSync(ENV.SKARBIEC_VAULT_FILE, 'utf8'));

const KEY_FIELDS = [
  ['key'], ['apiKey'], ['api_key'], ['access'], ['accessToken'], ['access_token'],
  ['token'], ['tokens', 'access_token'], ['claudeAiOauth', 'accessToken'],
];

function config() {
  const document = JSON.parse(skarbiec(['get', 'codex-reauth-config']));
  let value = document.fields.value;
  if (typeof value === 'string') value = JSON.parse(value);
  let metadata = value.metadata;
  if (typeof metadata === 'string') metadata = JSON.parse(metadata);
  return metadata;
}

function keyMaterial(secret) {
  const length = crypto.createHash('sha256').digest().length;
  const base64Length = Buffer.alloc(length).toString('base64').length;
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
  `${store}/rest/v1/trade_agent_subscriptions?status=eq.active&select=id,provider,key_encrypted,key_label,updated_at`,
  { headers: { apikey: metadata.MR_SUPABASE_SERVICE_ROLE_KEY, Authorization: `Bearer ${metadata.MR_SUPABASE_SERVICE_ROLE_KEY}` } },
);
if (!response.ok) throw new Error(`pool read -> HTTP ${response.status}`);
const pool = await response.json();

const row = pool.find((entry) => String(entry.key_label || '').includes(ACCOUNT));
if (!row) {
  console.log(`no active pool row whose label mentions ${JSON.stringify(ACCOUNT)}`);
  console.log('labels present:');
  for (const entry of pool) console.log(`  ${entry.provider}  ${entry.key_label}`);
  process.exit(Number(Boolean(process.env.STRICT)));
}

const provider = String(row.provider).replace(/_/g, '-');
const subscription = `brama-sub-${AGENT}-${SUFFIX}`;
const item = `provider:${provider}:${subscription}`;
console.log(`account: ${row.key_label}`);
console.log(`provider: ${provider}`);
console.log(`item:     ${item}`);

const plaintext = decrypt(material, row.key_encrypted);
const carried = credentialField(plaintext);
if (!carried) {
  console.log('the decrypted credential carries no field the adapter reads; refusing to create the item');
  process.exit(Number(Boolean(true)));
}
console.log(`credential: ${carried.field} (${carried.length} chars)`);

// Recipients and tags come from the sibling of the same provider: an item only
// this process can open, or one no agent is allowed to use, is not a
// subscription anybody can spend.
const items = vault().items || {};
const sibling = Object.entries(items).find(
  ([name, record]) => name.startsWith(`provider:${provider}:brama-sub-`) && name !== item && (record.recipients || []).length,
);
if (!sibling) {
  console.log(`no existing ${provider} item to copy recipients and tags from`);
  process.exit(Number(Boolean(true)));
}
const [siblingName, siblingRecord] = sibling;
const recipients = siblingRecord.recipients || [];
const tags = (siblingRecord.tags || []).map((tag) =>
  tag.startsWith('brama:id:') ? `brama:id:${subscription}` : tag,
);
console.log(`copying envelope from: ${siblingName}`);
console.log(`recipients: ${recipients.length}`);
console.log(`tags:       ${tags.join(',')}`);

if (!WRITE) {
  console.log('mode: reporting only (pass --write to create it)');
  process.exit(Number(Boolean(false)));
}
if (items[item]) {
  console.log('item already exists; leaving it untouched');
  process.exit(Number(Boolean(false)));
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
console.log(`created ${item}`);
