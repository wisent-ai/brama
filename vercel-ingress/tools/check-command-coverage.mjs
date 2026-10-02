#!/usr/bin/env node
// Every public command `brama` declares has a documentation route, or this
// refuses. The commands are the variants of `enum Commands` in src/main.rs,
// in clap's kebab-case spelling, minus the ones marked `#[command(hide =
// true)]`; the routes are the `/docs/cli/<command>` rewrites in vercel.json,
// each of which must point at an HTML file that exists under docs/. A group
// counts as documented when `/docs/cli/<group>` or any route below it is
// rewritten.
//
//   node vercel-ingress/tools/check-command-coverage.mjs
//
// Exit 0 when every command has a route, 1 with the missing ones named, 2 for
// a wrong invocation or an unreadable source.

import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ingress = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(ingress, "..");
if (process.argv.length > 2) {
  console.error(`check-command-coverage: unknown argument ${process.argv[2]}`);
  process.exit(2);
}

const mainPath = resolve(repo, "src/main.rs");
let main;
try {
  main = readFileSync(mainPath, "utf8");
} catch (error) {
  console.error(`check-command-coverage: ${mainPath} cannot be read (${error.message})`);
  process.exit(2);
}
const start = main.indexOf("enum Commands {");
const end = main.indexOf("\n}\n", start);
if (start < 0 || end < 0) {
  console.error(`check-command-coverage: ${mainPath} has no Commands enum`);
  process.exit(2);
}
const kebab = (variant) => variant.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
const declared = new Set();
let hidden = false;
for (const line of main.slice(start, end).split("\n")) {
  if (/^\s*#\[command\(hide = true\)\]/.test(line)) {
    hidden = true;
    continue;
  }
  const variant = line.match(/^ {4}([A-Z][A-Za-z0-9]*)\b/);
  if (!variant) continue;
  if (!hidden) declared.add(kebab(variant[1]));
  hidden = false;
}

const vercel = JSON.parse(readFileSync(resolve(ingress, "vercel.json"), "utf8"));
const routes = new Map();
for (const rewrite of vercel.rewrites ?? []) {
  if (typeof rewrite.source === "string" && rewrite.source.startsWith("/docs/cli/")) {
    routes.set(rewrite.source, rewrite.destination);
  }
}

const missing = [];
for (const command of [...declared].sort()) {
  const prefix = `/docs/cli/${command}`;
  const matching = [...routes.entries()].filter(([source]) => source === prefix || source.startsWith(`${prefix}/`));
  if (matching.length === 0) {
    missing.push(`${command} (no /docs/cli/${command} rewrite in vercel.json)`);
    continue;
  }
  for (const [source, destination] of matching) {
    if (!existsSync(resolve(ingress, `.${destination}`))) {
      missing.push(`${command} (${source} points at ${destination}, which does not exist)`);
    }
  }
}
if (missing.length) {
  console.error(`check-command-coverage: ${missing.length} public command(s) have no page: ${missing.join("; ")}`);
  process.exit(1);
}
console.log(`check-command-coverage: every one of ${declared.size} public commands has a documentation route`);
