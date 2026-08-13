#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Bank every donated credential into a subscription slot this host declares.
//
// The host's manifest declares far more subscriptions than the vault holds --
// four Claude slots among them -- and the donation pool holds credentials with
// nowhere to go. Both halves were right and nothing joined them: a credential
// with no item is invisible to the gateway, and a declared slot with no item is
// a capability nobody can spend.
//
// Slots come from the manifest the launcher actually reads, never from a list
// written here, so this cannot invent an id the signed policy has never heard
// of. Recipients and agent tags are copied from an existing item of the same
// provider, so no envelope is guessed and no permission widened. Populated
// slots are left alone.
//
// Installed as `bank-...` it writes; under any other name it only reports.

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
const MANIFEST = `${process.env.BRAMA_SKARBIEC_CONFIG_DIR || `${HOME}/.config/brama/trust`}/subscriptions.json`;
const [, invokedAs] = process.argv;
const WRITE = process.argv.includes('--write') || /(^|\/)bank-/.test(String(invokedAs));

const KEY_FIELDS = [
  ['key'], ['apiKey'], ['api_key'], ['access'], ['accessToken'], ['access_token'],
  ['token'], ['tokens', 'access_token'], ['claudeAiOauth', 'accessToken'],
];

const skarbiec = (args, input) => execFileSync(CLI, args, { env: ENV, encoding: 'utf8', input });
const vault = () => JSON.parse(readFileSync(ENV.SKARBIEC_VAULT_FILE, 'utf8'));

function routerConfig() {
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

const metadata = routerConfig();
const material = keyMaterial(metadata.ENCRYPTION_KEY);
const store = metadata.MR_SUPABASE_URL.replace(/\/+$/, '');
const response = await fetch(
  `${store}/rest/v1/trade_agent_subscriptions?status=eq.active&select=id,provider,key_encrypted,key_label,updated_at&order=updated_at.desc`,
  { headers: { apikey: metadata.MR_SUPABASE_SERVICE_ROLE_KEY, Authorization: `Bearer ${metadata.MR_SUPABASE_SERVICE_ROLE_KEY}` } },
);
if (!response.ok) throw new Error(`pool read -> HTTP ${response.status}`);
const pool = await response.json();

const declared = JSON.parse(readFileSync(MANIFEST, 'utf8'));
const items = vault().items || {};
const itemName = (provider, id) => `provider:${provider}:${id}`;

console.log(`manifest: ${MANIFEST} (${declared.length} declared)`);
console.log(`pool:     ${pool.length} active credentials`);
console.log(WRITE ? 'mode: writing' : 'mode: reporting only');

// The pool names a provider with underscores, the manifest with dashes.
const dashed = (provider) => String(provider).replace(/_/g, '-');

for (const provider of [...new Set(pool.map((row) => dashed(row.provider)))]) {
  const slots = declared
    .filter((entry) => dashed(entry.provider) === provider)
    .map((entry) => entry.id);
  const empty = slots.filter((id) => !items[itemName(provider, id)]);
  const rows = pool.filter((row) => dashed(row.provider) === provider);
  // Which credentials are already banked cannot be read from the vault file:
  // the document that records where a row came from is inside the ciphertext,
  // and only the envelope -- recipients and tags -- is in the clear. Comparing
  // a digest of the decrypted value against the populated slots is what keeps
  // this from writing a second copy of a credential that already has a home.
  const digest = (text) => crypto.createHash('sha256').update(text).digest('hex');
  const banked = new Set();
  for (const id of slots) {
    const name = itemName(provider, id);
    if (!items[name]) continue;
    try {
      const value = JSON.parse(skarbiec(['get', name])).fields.value;
      banked.add(digest(typeof value === 'string' ? value : JSON.stringify(value)));
    } catch {
      // An unreadable slot is not proof of anything; treat it as unoccupied.
    }
  }
  const homeless = rows.filter((row) => {
    try {
      return !banked.has(digest(decrypt(material, row.key_encrypted)));
    } catch {
      return true;
    }
  });

  console.log(`\n${provider}: ${slots.length} slot(s), ${empty.length} empty, ${rows.length} credential(s)`);
  if (!empty.length) {
    console.log('  every declared slot is populated; nothing to bank');
    continue;
  }

  const sibling = Object.entries(items).find(
    ([name, record]) => name.startsWith(`provider:${provider}:brama-sub-`) && (record.recipients || []).length,
  ) || Object.entries(items).find(
    ([name, record]) => name.includes(':brama-sub-') && (record.recipients || []).length,
  );
  if (!sibling) {
    console.log('  no existing subscription item to copy an envelope from');
    continue;
  }
  const [siblingName, siblingRecord] = sibling;

  for (const row of homeless) {
    const slot = empty.shift();
    if (!slot) {
      console.log(`  no empty slot left for ${row.key_label}`);
      break;
    }
    const target = itemName(provider, slot);
    let plaintext;
    try {
      plaintext = decrypt(material, row.key_encrypted);
    } catch (error) {
      console.log(`  ${slot}: does not decrypt (${error.code || error.name})`);
      continue;
    }
    const carried = credentialField(plaintext);
    if (!carried) {
      console.log(`  ${slot}: plaintext carries no field the adapter reads`);
      continue;
    }
    console.log(`  ${slot} <- ${JSON.stringify(row.key_label)}  [${carried.field}, ${carried.length} chars]`);
    if (!WRITE) continue;
    const recipients = siblingRecord.recipients || [];
    const tags = (siblingRecord.tags || []).map((tag) =>
      tag.startsWith('brama:id:') ? `brama:id:${slot}` : tag,
    );
    const body = JSON.stringify({
      schema: 'skarbiec.item.v2',
      kind: 'bundle',
      fields: { value: plaintext },
      context: { source: 'trade_agent_subscriptions', row: row.id, label: row.key_label },
    });
    const args = ['set-json', target, '--recipients', recipients.join(',')];
    if (tags.length) args.push('--tags', tags.join(','));
    skarbiec(args, body);
    console.log(`    written (envelope from ${siblingName})`);
  }
}
