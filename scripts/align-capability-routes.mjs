#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Give every allowed, banked subscription a route to its vault coordinate.
//
// A banked credential becomes spendable only after four things agree: the vault
// holds the item, the signed policy allows its resource, the routes table maps
// that resource to an item and field, and issuance succeeds. The third was
// missing for the slots filled from the donation pool, and its absence is
// silent -- the launcher drops the subscription with `continue`, so a healthy
// credential simply never appears in the catalogue any agent is served from.
//
// Routes are derived: for each item the policy already allows, the coordinate
// is the item itself and the field the adapter reads. Nothing is invented for a
// resource the policy has not named, so this cannot widen what may be issued.
//
// Installed as `align-...` it writes; under any other name it only reports.

import os from 'node:os';
import { readFileSync, writeFileSync } from 'node:fs';

const HOME = os.homedir();
const TRUST = process.env.BRAMA_SKARBIEC_CONFIG_DIR || `${HOME}/.config/brama/trust`;
const ROUTES = process.env.SKARBIEC_CAPABILITY_ROUTES_FILE || `${HOME}/.stado/capability-routes.json`;
const VAULT = process.env.SKARBIEC_VAULT_FILE || `${HOME}/.stado/skarbiec.vault.json`;
const FIELD = 'value';
const [, invokedAs] = process.argv;
const WRITE = process.argv.includes('--write') || /(^|\/)align-/.test(String(invokedAs));

const policy = JSON.parse(readFileSync(`${TRUST}/policy.json`, 'utf8'));
const allowed = new Set(
  (policy.roles?.['brama-runtime'] ?? [])
    .filter((rule) => rule && rule.purpose === 'brama.provider.authenticate')
    .map((rule) => rule.resource)
    .filter((resource) => typeof resource === 'string' && resource.split(':').length === 3),
);

const items = JSON.parse(readFileSync(VAULT, 'utf8')).items ?? {};
const banked = Object.keys(items).filter((name) => name.includes(':brama-sub-') && !items[name].deleted);

const routes = JSON.parse(readFileSync(ROUTES, 'utf8'));

console.log(`policy allows ${allowed.size} scoped subscription(s)`);
console.log(`vault holds   ${banked.length} subscription item(s)`);
console.log(`routes file:  ${ROUTES}`);
console.log(WRITE ? 'mode: writing' : 'mode: reporting only');

let added = 0;
for (const name of banked.sort()) {
  if (!allowed.has(name)) {
    console.log(`  ${name}: not allowed by the policy; no route added`);
    continue;
  }
  if (routes[name]) {
    continue;
  }
  console.log(`  ${name}: missing a route -> ${name}#${FIELD}`);
  routes[name] = { item: name, field: FIELD };
  added += 1;
}

if (!added) {
  console.log('every allowed, banked subscription already has a route');
} else if (WRITE) {
  writeFileSync(ROUTES, `${JSON.stringify(routes, null, 2)}\n`, { mode: 0o600 });
  console.log(`added ${added} route(s)`);
} else {
  console.log(`${added} route(s) would be added`);
}
