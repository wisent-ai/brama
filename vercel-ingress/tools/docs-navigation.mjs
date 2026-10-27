#!/usr/bin/env node
// Every documentation page wears one navigation, rendered from one source:
// the "All topics" directory on the docs home page (docs/index.html). A
// person changes the navigation there and nowhere else; this tool renders it
// into every routed page's mobile menu and sidebar, marking the page itself.
// It also drops a "Documentation topic:" source note naming a file the
// repository no longer has: the pages are the source.
//
//   node vercel-ingress/tools/docs-navigation.mjs check
//   node vercel-ingress/tools/docs-navigation.mjs write
//
// check names every page whose navigation differs from the rendering, every
// page that wears none, every stale source note, every directory link no
// rewrite in vercel.json serves, and every page file under docs/ that no
// rewrite serves (it is published nowhere and keeps whatever navigation it
// was written with). Sub-pages are reached from their parent's page, so the
// directory lists topics, not every route. write rewrites the navigation and
// removes the stale notes in place, then names what it cannot place itself:
// a page without navigation lines, a link to nothing and a page no route
// serves. A page wearing its section's navigation (a runbook or readme topic
// split into sub-pages) keeps it; check names any link in it nothing serves.

import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Exit statuses as POSIX comparison utilities state them: success, a finding,
// and an unusable invocation or input.
const FINDING = 1; // https://pubs.opengroup.org/onlinepubs/9699919799/utilities/cmp.html#tag_20_19_14
const UNUSABLE = 2; // https://pubs.opengroup.org/onlinepubs/9699919799/utilities/cmp.html#tag_20_19_14

const ingress = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(ingress, "..");
const MOBILE = '<details class="mobile-nav">';
const SIDEBAR = '<aside aria-label="Documentation navigation">';
// A topic split into a parent page and sub-pages wears that section's own
// navigation instead of the directory; its links must still be served.
const SECTION = '<aside aria-label="Section navigation">';
const GRID_OPEN = '<div class="directory-grid">';
const GRID_CLOSE = "</div></section>";
const NOTE_OPEN = '<p class="source-note">Documentation topic: ';
const NOTE_CLOSE = "</p>";

// The command's own verbs: what each does with a finding it can repair.
const VERBS = {
  check: { writes: false, drift: "differs from the directory", note: "present", repairable: true },
  write: { writes: true, drift: "rewritten", note: "removed", repairable: false },
};

function refuse(message) {
  console.error(`docs-navigation: ${message}`);
  process.exit(UNUSABLE);
}

function read(path) {
  try {
    return readFileSync(path, "utf8");
  } catch (error) {
    refuse(`${relative(repo, path)} cannot be read (${error.message})`);
  }
}

const [, , verb, ...extra] = process.argv;
if (!Object.hasOwn(VERBS, verb) || extra.length) {
  refuse(`usage: docs-navigation.mjs ${Object.keys(VERBS).join("|")}`);
}
const mode = VERBS[verb];

// Routes: every vercel.json rewrite onto a docs HTML file. A rewrite whose
// last segment is a parameter (/docs/providers/:slug onto
// /docs/providers/:slug.html) serves every page file in that directory, so
// each one is a route; a parameter anywhere else is not a page layout this
// tool can place.
let rewrites;
try {
  ({ rewrites } = JSON.parse(read(join(ingress, "vercel.json"))));
} catch (error) {
  refuse(`vercel.json is not JSON (${error.message})`);
}
if (!Array.isArray(rewrites)) refuse("vercel.json declares no rewrites array");

function served(source, destination) {
  if (!destination.includes(":")) return [[source, destination]];
  const directory = dirname(destination);
  const parameter = basename(destination, ".html");
  if (directory.includes(":") || !parameter.startsWith(":") || !source.endsWith(`/${parameter}`)) {
    refuse(`vercel.json rewrites ${source} onto ${destination}, a parameter outside the page's own name`);
  }
  let names;
  try {
    names = readdirSync(join(ingress, directory));
  } catch (error) {
    refuse(`${relative(repo, join(ingress, directory))} cannot be listed (${error.message})`);
  }
  return names
    .filter((name) => name.endsWith(".html"))
    .map((name) => [
      `${source.slice(0, source.length - parameter.length)}${basename(name, ".html")}`,
      `${directory}/${name}`,
    ]);
}

const pages = new Map();
for (const { source, destination } of rewrites) {
  if (typeof destination === "string" && destination.startsWith("/docs/") && destination.endsWith(".html")) {
    for (const [route, file] of served(source, destination)) {
      if (!pages.has(route)) pages.set(route, join(ingress, file));
    }
  }
}

// The directory: groups of links on the home page.
const home = pages.get("/docs");
if (!home) refuse("vercel.json rewrites no /docs home page");
const index = read(home);
if (!index.includes(GRID_OPEN)) refuse(`${relative(repo, home)} has no ${GRID_OPEN}`);
const gridStart = index.indexOf(GRID_OPEN) + GRID_OPEN.length;
if (!index.includes(GRID_CLOSE, gridStart)) refuse(`${relative(repo, home)} never closes the directory`);
const grid = index.slice(gridStart, index.indexOf(GRID_CLOSE, gridStart));
const groups = grid
  .split("<section><h2>")
  .filter((part) => part.trim())
  .map((part) => {
    if (!part.includes("</h2>")) refuse(`a directory group in ${relative(repo, home)} has no heading: ${part}`);
    const [title] = part.split("</h2>");
    const links = [...part.matchAll(/<a href="([^"]+)">([^<]+)<\/a>/g)].map(([, href, label]) => ({ href, label }));
    if (!links.length) refuse(`directory group "${title}" in ${relative(repo, home)} lists no page`);
    return { title, links };
  });

function rendered(here) {
  return groups
    .map(({ title, links }) => {
      const anchors = links
        .map(({ href, label }) =>
          href === here
            ? `<a class="active" aria-current="page" href="${href}">${label}</a>`
            : `<a href="${href}">${label}</a>`,
        )
        .join("");
      return `<section class="nav-group"><h2>${title}</h2>${anchors}</section>`;
    })
    .join("");
}

function lineOf(lines, opener) {
  for (const [at, line] of lines.entries()) {
    if (line.trimStart().startsWith(opener)) return at;
  }
  return undefined;
}

function staleNote(line) {
  const trimmed = line.trim();
  if (!trimmed.startsWith(NOTE_OPEN) || !trimmed.endsWith(NOTE_CLOSE)) return false;
  const named = trimmed.slice(NOTE_OPEN.length, trimmed.length - NOTE_CLOSE.length);
  return !existsSync(join(repo, named)) && !existsSync(join(ingress, named));
}

// Every routed page is read before any is written, so a page that cannot be
// read refuses the run with no page rewritten and none left half done.
const drifted = [];
const bare = [];
const notes = [];
const rewritten = [];
const sectionLinks = [];
for (const [route, file] of pages) {
  const text = read(file);
  const lines = text.split("\n");
  const body = rendered(route);
  const mobileAt = lineOf(lines, MOBILE);
  const sidebarAt = lineOf(lines, SIDEBAR);
  const sectionAt = lineOf(lines, SECTION);
  if ((mobileAt === undefined || sidebarAt === undefined) && sectionAt !== undefined) {
    for (const [, href] of lines[sectionAt].matchAll(/<a[^>]* href="(\/docs[^"#]*)"/g)) {
      sectionLinks.push({ route, href });
    }
  } else if (mobileAt === undefined || sidebarAt === undefined) {
    bare.push(route);
  } else {
    const mobile = `  ${MOBILE}<summary>Browse documentation</summary><nav>${body}</nav></details>`;
    const sidebar = `    ${SIDEBAR}<nav>${body}</nav></aside>`;
    if (lines[mobileAt] !== mobile || lines[sidebarAt] !== sidebar) {
      drifted.push(route);
      lines[mobileAt] = mobile;
      lines[sidebarAt] = sidebar;
    }
  }
  const kept = lines.filter((line) => !staleNote(line));
  if (kept.length !== lines.length) notes.push(route);
  if (kept.join("\n") !== text) rewritten.push([file, kept.join("\n")]);
}
if (mode.writes) {
  for (const [file, text] of rewritten) writeFileSync(file, text);
}

const listed = new Set(groups.flatMap(({ links }) => links.map(({ href }) => href)));
const dangling = [
  ...[...listed].filter((href) => !pages.has(href)),
  ...sectionLinks.filter(({ href }) => !pages.has(href)).map(({ route, href }) => `${href} (section navigation of ${route})`),
];

function pageFiles(directory) {
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    refuse(`${relative(repo, directory)} cannot be listed (${error.message})`);
  }
  return entries.flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return pageFiles(path);
    return entry.name.endsWith(".html") ? [path] : [];
  });
}
const routedFiles = new Set(pages.values());
const unrouted = pageFiles(join(ingress, "docs")).filter((file) => !routedFiles.has(file));

for (const route of drifted) console.log(`${route}: navigation ${mode.drift}`);
for (const route of notes) console.log(`${route}: source note naming a missing file ${mode.note}`);
for (const route of bare) console.log(`${route}: wears no navigation lines; place the page in the layout by hand`);
for (const href of dangling) console.log(`${href}: listed in the directory but no vercel.json rewrite serves it`);
for (const file of unrouted) {
  console.log(`${relative(repo, file)}: no vercel.json rewrite serves this page; route it or delete it`);
}
// Drift and missing-file notes stay findings in a check; a write repaired them.
const pending = mode.repairable ? [...drifted, ...notes] : [];
if (bare.length || dangling.length || unrouted.length || pending.length) process.exit(FINDING);
