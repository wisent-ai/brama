// Measures Brama's real command surface and checks the documentation covering it, replacing a hand-typed
// list of thirteen command pages that went stale the moment `adopt` and `aliases` shipped and could not have
// noticed. Every number comes from the binary named by BRAMA_BIN, the router in src/core/server.rs and the
// rewrite table in vercel-ingress/vercel.json. It is the tool /docs/command-surface names, it writes nothing:
//   BRAMA_BIN=target/release/brama node --test tests/docs/cli-commands.probierz.spec.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
const REPOSITORY = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const INGRESS = join(REPOSITORY, "vercel-ingress");
const PRODUCTION_ORIGIN = "https://brama.wisent.com";
const METHODS = ["get", "post", "put", "delete", "patch", "head", "options"];
// The root `help` is public and documented; clap's copies inside every group are one generated node per group.
const GENERATED_HELP = "help";
// clap wraps to the terminal width, so the width is pinned and colour disabled: the parse below reads columns.
const HELP_ENV = { COLUMNS: "240", NO_COLOR: "1" };
// `brama help <tokens>`, not `<tokens> --help`: clap's own `help` leaf rejects `--help`.
export function runHelp(binary, argv) {
  return execFileSync(binary, ["help", ...argv], {
    encoding: "utf8",
    env: { ...process.env, ...HELP_ENV },
  }).replaceAll("\r\n", "\n");
}
export function buildIdentity(binary) {
  const identity = JSON.parse(
    execFileSync(binary, ["version"], { encoding: "utf8", env: { ...process.env, NO_COLOR: "1" } }),
  );
  const missing = ["product", "version", "source_revision", "platform"].filter((f) => !identity[f]);
  if (missing.length > 0) throw new Error(`brama version printed no ${missing.join(", ")}`);
  return identity;
}
function section(help, name) {
  const lines = help.split("\n");
  const start = lines.findIndex((line) => line === `${name}:`);
  if (start < 0) return [];
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => /^[A-Za-z][A-Za-z ]+:$/.test(line));
  return end < 0 ? rest : rest.slice(0, end);
}
function rows(help, name, pattern) {
  return section(help, name)
    .map((line) => line.match(pattern))
    .filter((match) => match !== null)
    .map((match) => match[1].trim());
}
export function parseHelp(help, tokens = []) {
  const lines = help.split("\n");
  const start = lines.findIndex((line) => line.startsWith("Usage:"));
  if (start < 0) throw new Error(`brama ${tokens.join(" ")} --help printed no Usage line`);
  const usage = [lines[start].slice("Usage:".length).trim()];
  for (const line of lines.slice(start + 1)) {
    if (!line.trim() || /^[A-Za-z][A-Za-z ]+:$/.test(line)) break;
    usage.push(line.trim());
  }
  return {
    invocation: usage.join(" ").replace(/\s+/g, " "),
    subcommands: rows(help, "Commands", /^\s{2,}([a-zA-Z0-9][a-zA-Z0-9-]*)(?:\s{2,}|$)/),
    options: rows(help, "Options", /^\s{2,}(-\S(?:.*?\S)?)(?:\s{2,}|$)/)
      .filter((syntax) => !/(^|,\s*)-(?:h|-help|V|-version)\b/.test(syntax)),
  };
}
// A node with a `Commands:` section is a group; a node without one is a leaf.
export function walkCommandTree(readHelp) {
  const nodes = [];
  const visit = (tokens) => {
    const parsed = parseHelp(readHelp(tokens), tokens);
    const children = parsed.subcommands.filter((n) => !(n === GENERATED_HELP && tokens.length > 0));
    const kind = parsed.subcommands.length > 0 ? "group" : "leaf";
    nodes.push({ tokens, ...parsed, children, kind });
    for (const child of children) visit([...tokens, child]);
  };
  visit([]);
  return nodes;
}
// `brama onboard --adopt-from` is the whole `adopt` command at a second entry point.
export function optionMirrors(leaves) {
  const names = leaves.map((leaf) => leaf.tokens.at(-1)).filter((name) => name !== undefined);
  return leaves.flatMap((host) => names
    .filter((name) => name !== host.tokens.at(-1))
    .map((name) => ({
      host: host.tokens.join(" "),
      mirrored: name,
      options: host.options.filter((syntax) => syntax.startsWith(`--${name}-`)),
    }))
    .filter((mirror) => mirror.options.length > 1));
}
// Balanced-paren scanning, not one regex: `.route("/p", get(a).post(b).delete(c))` spans lines.
export function parseRouterInvocations(source) {
  const invocations = [];
  const marker = ".route(";
  for (let at = source.indexOf(marker); at >= 0; at = source.indexOf(marker, at + 1)) {
    let depth = 0;
    let end = -1;
    for (let scan = at + marker.length - 1; scan < source.length && end < 0; scan += 1) {
      if (source[scan] === "(") depth += 1;
      else if (source[scan] === ")" && --depth === 0) end = scan;
    }
    if (end < 0) throw new Error(`unbalanced .route( at offset ${at}`);
    const args = source.slice(at + marker.length, end);
    const path = args.match(/"([^"]+)"/);
    if (path === null) throw new Error(`.route( at offset ${at} names no path`);
    const methods = [...args.matchAll(/\b([a-z]+)\s*\(/g)]
      .map((match) => match[1])
      .filter((name) => METHODS.includes(name));
    if (methods.length === 0) throw new Error(`.route("${path[1]}") names no HTTP method`);
    for (const method of methods) {
      invocations.push({ method: method.toUpperCase(), path: path[1], group: httpGroup(path[1]) });
    }
    at = end;
  }
  return invocations;
}
export function httpGroup(path) {
  if (path.startsWith("/v1/admin/")) return "/v1/admin";
  if (path.startsWith("/v1/account/")) return "/v1/account";
  return path.startsWith("/v1/") ? "/v1" : "/ (public and process)";
}
export function readDocsRegistry(ingress = INGRESS) {
  const config = JSON.parse(readFileSync(join(ingress, "vercel.json"), "utf8"));
  assert.ok(Array.isArray(config.rewrites), "vercel.json declares no rewrites");
  const docs = config.rewrites.filter((r) => typeof r.source === "string"
    && typeof r.destination === "string" && r.destination.startsWith("/docs/"));
  return new Map(docs.map((r) => [r.source, r.destination]));
}
function pageFor(route, registry, ingress) {
  const destination = registry.get(route);
  if (destination === undefined) return null;
  const page = join(ingress, destination.replace(/^\//, ""));
  return existsSync(page) ? page : null;
}
export const FAMILIES = [
  {
    name: "credential repair, once per incident",
    capability: "subscription credential state",
    declaration: "the usage ledger's credential state and the journal's subscription_refresh records",
    matches: ({ path = "", tokens = [] }) => /sign-in|-pool\/refresh$|\/probe$/.test(path)
      || (tokens[0] === "subscription" && ["refresh", "sign-in"].includes(tokens[1])),
  },
  {
    name: "plan-usage reading, once per audience",
    capability: "subscription plan usage",
    declaration: "the usage ledger and the provider's own usage report",
    matches: ({ path = "" }) => /subscription-usage|-pool\/usage$/.test(path),
  },
  {
    name: "subscription inventory, once per audience",
    capability: "the subscription pool",
    declaration: "the vault entitlement declaration: brama:subscription with its agent, provider and id tags",
    matches: ({ path = "", tokens = [] }) => /^\/v1\/(subscriptions|account\/subscriptions|admin\/subscription)/.test(path)
      || (tokens[0] === "subscriptions" && tokens[1] === "list"),
  },
  {
    name: "route adoption, once per entry point",
    capability: "the alias registry",
    declaration: "the route registry BRAMA_INFERENCE_ROUTES_FILE names and the launcher table BRAMA_MODEL_ALIASES",
    matches: ({ path = "", tokens = [] }) => /configuration-adoption|^\/v1\/admin\/routes$/.test(path)
      || tokens[0] === "adopt",
  },
  {
    name: "provider credential administration",
    capability: "capability redemption at final use",
    declaration: "the provider capability declaration this installation was given",
    matches: ({ path = "" }) => path === "/v1/admin/credentials",
  },
];
export function classify(invocation) {
  const family = FAMILIES.find((one) => one.matches(invocation));
  if (family === undefined) return null;
  return family.name;
}
export function measure({ binary, repository = REPOSITORY, ingress = INGRESS, readHelp = (t) => runHelp(binary, t) } = {}) {
  const nodes = walkCommandTree(readHelp);
  const registry = readDocsRegistry(ingress);
  const leaves = nodes.filter((node) => node.kind === "leaf");
  const groups = nodes.filter((node) => node.kind === "group");
  const cliRoute = (tokens) => `/docs/cli/${tokens.join("/")}`.replace(/\/$/, "");
  const cli = leaves.map((leaf) => ({
    name: `brama ${leaf.tokens.join(" ")}`,
    invocation: leaf.invocation,
    options: leaf.options.length,
    route: cliRoute(leaf.tokens),
    documented: pageFor(cliRoute(leaf.tokens), registry, ingress) !== null,
    family: classify({ tokens: leaf.tokens }),
  }));
  const apiPage = pageFor("/docs/http-api", registry, ingress);
  if (apiPage === null) throw new Error("the docs registry has no resolving /docs/http-api page");
  const apiText = readFileSync(apiPage, "utf8");
  const router = readFileSync(join(repository, "src/core/server.rs"), "utf8");
  const http = parseRouterInvocations(router).map((one) => ({
    ...one,
    name: `${one.method} ${one.path}`,
    documented: apiText.includes(one.path),
    family: classify(one),
  }));
  const httpGroups = new Map();
  for (const one of http) {
    if (!httpGroups.has(one.group)) httpGroups.set(one.group, 0);
    httpGroups.set(one.group, httpGroups.get(one.group) + 1);
  }
  const all = [...cli, ...http];
  const named = (list) => list.map((one) => one.name);
  return {
    identity: buildIdentity(binary),
    cli,
    http,
    mirrors: optionMirrors(leaves),
    families: FAMILIES.map((f) => ({ ...f, members: named(all.filter((o) => o.family === f.name)) })),
    primitives: named(all.filter((one) => one.family === null)),
    undocumented: named(all.filter((one) => !one.documented)),
    // A parameterised rewrite resolves per request, so it has no single page file to find.
    unrouted: [...registry.keys()].filter((r) => !r.includes(":") && pageFor(r, registry, ingress) === null),
    groups: [
      ...groups.map((g) => ({
        name: g.tokens.length > 0 ? `brama ${g.tokens.join(" ")}` : "brama (root)",
        route: cliRoute(g.tokens),
        count: g.children.length,
      })),
      ...[...httpGroups.entries()].map(([name, count]) => ({ name, count })),
    ].sort((left, right) => right.count - left.count || left.name.localeCompare(right.name)),
    counts: {
      leaves: all.length,
      cliLeaves: cli.length,
      cliGroups: groups.length,
      cliOptions: cli.reduce((total, leaf) => total + leaf.options, 0),
      httpInvocations: http.length,
      httpPaths: new Set(http.map((one) => one.path)).size,
      groups: groups.length + httpGroups.size,
      registeredDocsRoutes: registry.size,
    },
  };
}
export function formatReport(m) {
  return [
    `brama ${m.identity.version} at ${m.identity.source_revision} (${m.identity.platform})`,
    `${m.counts.leaves} leaf invocations in ${m.counts.groups} groups`,
    `  argv: ${m.counts.cliLeaves} leaves, ${m.counts.cliGroups} groups, ${m.counts.cliOptions} options`,
    `  http: ${m.counts.httpInvocations} method invocations over ${m.counts.httpPaths} paths`,
    `${m.undocumented.length} with no prose page; ${m.counts.registeredDocsRoutes} docs routes registered, ${m.unrouted.length} with no page file`,
    "groups:",
    ...m.groups.map((g) => `  ${String(g.count).padStart(3)}  ${g.name}`),
    "families:",
    ...m.families.flatMap((f) => [
      `  ${String(f.members.length).padStart(3)}  ${f.name} -> ${f.capability} (reads ${f.declaration})`,
      ...f.members.map((one) => `        ${one}`),
    ]),
    `  ${String(m.primitives.length).padStart(3)}  primitives that stay: ${m.primitives.join(", ")}`,
    ...m.mirrors.map((mi) => `mirrored: brama ${mi.host} carries ${mi.options.length} ${mi.mirrored} options`),
    ...(m.undocumented.length > 0 ? [`undocumented: ${m.undocumented.join(", ")}`] : []),
    ...(m.unrouted.length > 0 ? [`unrouted pages: ${m.unrouted.join(", ")}`] : []),
  ].join("\n");
}
let cached;
function surface() {
  const binary = process.env.BRAMA_BIN;
  if (!binary) throw new Error("BRAMA_BIN must name the exact brama binary being measured");
  if (!existsSync(binary)) throw new Error(`BRAMA_BIN does not exist: ${binary}`);
  if (cached === undefined) cached = measure({ binary });
  return cached;
}
test("the measured command surface is reported at an identified revision", (t) => {
  const m = surface();
  t.diagnostic(`\n${formatReport(m)}`);
  assert.notEqual(m.identity.source_revision, "development", "a published count needs a named revision");
  const grouped = m.families.reduce((total, family) => total + family.members.length, 0);
  assert.equal(grouped + m.primitives.length, m.counts.leaves, "every invocation is grouped or a primitive");
});
test("documentation covers exactly the commands the binary has", () => {
  const m = surface();
  assert.deepEqual(
    m.cli.filter((leaf) => !leaf.documented).map((leaf) => leaf.route),
    [],
    "a command with no resolving page is undiscoverable to whoever has to run it",
  );
  const commandRoutes = [...m.cli, ...m.groups].map((one) => one.route);
  const routes = new Set(commandRoutes.filter((route) => route !== undefined));
  assert.deepEqual(
    [...readDocsRegistry().keys()].filter((s) => s.startsWith("/docs/cli/") && !routes.has(s)),
    [],
    "a page for a removed command keeps promising it",
  );
});
test("the production site serves every measured command page", async (t) => {
  for (const route of [...surface().cli.map((leaf) => leaf.route), "/docs/command-surface"]) {
    await t.test(route, async () => {
      const expected = new URL(route, PRODUCTION_ORIGIN).href;
      const response = await fetch(expected, { redirect: "follow" });
      assert.equal(response.status, 200, `${expected} must return 200`);
      assert.equal(response.url, expected, `${route} must remain canonical`);
      const escaped = expected.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      assert.match(
        await response.text(),
        new RegExp(`<link\\s+rel="canonical"\\s+href="${escaped}"`),
        `${route} must declare its canonical URL`,
      );
    });
  }
});
