#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Remove subscription items the host's manifest does not declare.
//
// The gateway discovers a subscription from any vault item named
// `brama-sub-<agent>-*`, but it can only spend one the signed capability policy
// covers, and that policy is generated from the manifest. An item outside the
// manifest is therefore discovered, attempted, refused, and recorded as a
// blocked credential -- noise that looks exactly like a broken subscription.
//
// Installed as `remove-...` it deletes; under any other name it only reports.

import { createHash } from 'node:crypto';
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
const WRITE = process.argv.includes('--write') || /(^|\/)remove-/.test(String(invokedAs));

const declared = JSON.parse(readFileSync(MANIFEST, 'utf8'));
const allowed = new Set(
  declared.map((entry) => `provider:${String(entry.provider).replace(/_/g, '-')}:${entry.id}`),
);
const items = JSON.parse(readFileSync(ENV.SKARBIEC_VAULT_FILE, 'utf8')).items || {};

console.log(`manifest: ${MANIFEST} (${declared.length} declared)`);
console.log(WRITE ? 'mode: deleting' : 'mode: reporting only');

// An undeclared item is removed only when the credential it holds is already
// banked in a declared slot. Undeclared and unique means the declaration is
// what is missing, not the item, and deleting it would throw away the only
// copy of something the fleet pays for.
const digest = (name) => {
  const value = JSON.parse(execFileSync(CLI, ['get', name], { env: ENV, encoding: 'utf8' })).fields.value;
  return createHash('sha256').update(typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
};

const bankedInDeclared = new Set();
for (const name of Object.keys(items)) {
  if (!allowed.has(name)) continue;
  try {
    bankedInDeclared.add(digest(name));
  } catch {
    // Unreadable here is not evidence; it simply cannot vouch for a duplicate.
  }
}

let found = false;
for (const name of Object.keys(items).sort()) {
  if (!name.includes(':brama-sub-') || allowed.has(name)) continue;
  found = true;
  let duplicate = false;
  try {
    duplicate = bankedInDeclared.has(digest(name));
  } catch {
    duplicate = false;
  }
  if (!duplicate) {
    console.log(`  undeclared, unique: ${name}  -> kept; the manifest is what is missing`);
    continue;
  }
  console.log(`  undeclared duplicate: ${name}`);
  if (!WRITE) continue;
  execFileSync(CLI, ['delete', name], { env: ENV, encoding: 'utf8' });
  console.log('    deleted');
}
if (!found) console.log('  every subscription item is declared');
