import assert from "node:assert/strict";
import { test } from "node:test";

const PRODUCTION_ORIGIN = "https://brama.wisent.com";

const commands = [
  ["/docs/cli/version", "brama version"],
  ["/docs/cli/serve", "brama serve [-p <PORT>|--port <PORT>] [--local-credentials-stdin]"],
  ["/docs/cli/onboard", "brama onboard [-m <ROUTE>|--model <ROUTE>] [--agent-id <ID>] [--allow-provider-cost]"],
  ["/docs/cli/test", "brama test [-m <ROUTE>|--model <ROUTE>] [--agent-id <ID>] --allow-provider-cost"],
  ["/docs/cli/detect", "brama detect"],
  ["/docs/cli/mcp", "brama mcp"],
  ["/docs/cli/subscriptions", "brama subscriptions <COMMAND>"],
  ["/docs/cli/subscriptions/list", "brama subscriptions list [--json]"],
  ["/docs/cli/subscription", "brama subscription <COMMAND>"],
  ["/docs/cli/subscription/refresh", "brama subscription refresh <PROVIDER> --reason <REASON> [--json]"],
  [
    "/docs/cli/subscription/sign-in",
    "brama subscription sign-in <PROVIDER> --reason <REASON> [--login-item <LOGIN_ITEM>] [--login-timeout-ms <MILLISECONDS>] [--json]",
  ],
  [
    "/docs/cli/collect-task-quality",
    "brama collect-task-quality --agent-id <ID> --task <KEY> --prompt <TEXT> [--expected-exact <TEXT>] [--expected-contains <TEXT>] [--persist] [--max-models <N>] --allow-provider-cost",
  ],
  ["/docs/cli/help", "brama help [COMMAND]..."],
];

function visibleText(html) {
  return html
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&")
    .replaceAll("&#39;", "'")
    .replaceAll("&quot;", '"')
    .replace(/\s+/g, " ")
    .trim();
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

test("every public Brama CLI command has a production documentation route", async (t) => {
  for (const [path, invocation] of commands) {
    await t.test(path, async () => {
      const expectedUrl = new URL(path, PRODUCTION_ORIGIN).href;
      const response = await fetch(expectedUrl, { redirect: "follow" });
      assert.equal(response.status, 200, `${expectedUrl} must return 200`);
      assert.equal(response.url, expectedUrl, `${path} must remain at its canonical route`);

      const html = await response.text();
      assert.match(
        html,
        new RegExp(`<link\\s+rel="canonical"\\s+href="${escapeRegExp(expectedUrl)}"\\s*>`),
        `${path} must declare its canonical URL`,
      );
      assert.ok(
        visibleText(html).includes(invocation),
        `${path} must document the exact invocation: ${invocation}`,
      );
    });
  }
});

test("the production CLI index links the complete command tree", async () => {
  const indexUrl = new URL("/docs/cli", PRODUCTION_ORIGIN).href;
  const response = await fetch(indexUrl, { redirect: "follow" });
  assert.equal(response.status, 200, `${indexUrl} must return 200`);
  assert.equal(response.url, indexUrl, "/docs/cli must remain canonical");

  const html = await response.text();
  for (const [path] of commands) {
    assert.match(
      html,
      new RegExp(`href="${escapeRegExp(path)}"`),
      `/docs/cli must link ${path}`,
    );
  }
});
