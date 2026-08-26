#!/usr/bin/env python3
"""Render the per-provider documentation pages from the provider table itself.

The docs site is hand-authored HTML under `vercel-ingress/docs`, and a page per
provider is exactly the kind of content that silently drifts: 23 descriptors,
each with a base URL, wire protocol, auth header shape, trusted host and
override variable that all live in `src/providers/adapter.rs` and change there
first. So these pages are not written by hand. This script parses the source
of truth -- the `PROVIDERS` table, `trusted_provider_hosts`,
`PLAN_USAGE_ENDPOINTS` in `adapter.rs`, and `oauth_provider` in
`gateway/oauth_refresh.rs` -- and rewrites `vercel-ingress/docs/providers.html`
plus one `vercel-ingress/docs/providers/<id>.html` per descriptor. The page
chrome (head, styles, sidebar) is taken from the published
`concepts/provider.html`, so the pages always match the site.

Run it after changing the provider table:

    python3 scripts/generate-provider-docs.py

Deterministic: same sources in, same bytes out. No network, no secrets --
everything rendered is already public in the repository.
"""

from __future__ import annotations

import html
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
ADAPTER = REPO / "src" / "providers" / "adapter.rs"
OAUTH = REPO / "src" / "gateway" / "oauth_refresh.rs"
DOCS = REPO / "vercel-ingress" / "docs"
TEMPLATE_PAGE = DOCS / "concepts" / "provider.html"

WIRE_LABEL = {
    "OpenAiChat": "openai-chat",
    "AnthropicMessages": "anthropic-messages",
    "OpenAiResponses": "openai-responses",
}

AUTH_LABEL = {
    "Bearer": "<code>Authorization: Bearer &lt;credential&gt;</code>",
    "XApiKey": (
        "<code>x-api-key: &lt;credential&gt;</code> with "
        "<code>anthropic-version: 2023-06-01</code>"
    ),
    "AnthropicBearer": (
        "<code>Authorization: Bearer &lt;OAuth access token&gt;</code> with "
        "<code>anthropic-version: 2023-06-01</code> and "
        "<code>anthropic-beta: oauth-2025-04-20</code>"
    ),
}


def parse_providers(source: str) -> list[dict]:
    """The `PROVIDERS` table, field by field, in declaration order."""
    table = re.search(
        r"const PROVIDERS: &\[ProviderDescriptor\] = &\[(.*?)\n\];", source, re.S
    )
    if not table:
        sys.exit("adapter.rs no longer declares `const PROVIDERS` -- update this parser")
    providers = []
    for block in re.finditer(r"ProviderDescriptor \{(.*?)\n    \},", table.group(1), re.S):
        body = block.group(1)

        def field(name: str) -> str:
            found = re.search(rf'{name}: "([^"]*)"', body)
            if not found:
                sys.exit(f"a descriptor is missing `{name}` -- update this parser")
            return found.group(1)

        wire = re.search(r"wire: WireProtocol::(\w+)", body)
        auth = re.search(r"auth: AuthKind::(\w+)", body)
        models = re.search(r"static_models: &\[(.*?)\]", body, re.S)
        if not (wire and auth and models):
            sys.exit("a descriptor is missing wire/auth/static_models -- update this parser")
        providers.append(
            {
                "id": field("id"),
                "display_name": field("display_name"),
                "base_url": field("base_url"),
                "models_path": field("models_path"),
                "chat_path": field("chat_path"),
                "wire": WIRE_LABEL[wire.group(1)],
                "auth": AUTH_LABEL[auth.group(1)],
                "static_models": re.findall(r'"([^"]+)"', models.group(1)),
            }
        )
    return providers


def parse_trusted_hosts(source: str) -> dict[str, list[str]]:
    """`trusted_provider_hosts`: which non-loopback hosts an override may name."""
    body = re.search(
        r"fn trusted_provider_hosts\(provider_id: &str\).*?match provider_id \{(.*?)\n\s*_ =>",
        source,
        re.S,
    )
    if not body:
        sys.exit("adapter.rs no longer has `trusted_provider_hosts` -- update this parser")
    hosts: dict[str, list[str]] = {}
    for arm in re.finditer(r'((?:"[\w-]+"\s*\|?\s*)+)=>\s*Some\(&\[(.*?)\]\)', body.group(1)):
        ids = re.findall(r'"([\w-]+)"', arm.group(1))
        row = re.findall(r'"([^"]+)"', arm.group(2))
        for provider_id in ids:
            hosts[provider_id] = row
    return hosts


def parse_plan_usage(source: str) -> dict[str, str]:
    """`PLAN_USAGE_ENDPOINTS`: the three providers that publish a usage report."""
    body = re.search(
        r"const PLAN_USAGE_ENDPOINTS: &\[PlanUsageEndpoint\] = &\[(.*?)\n\];", source, re.S
    )
    if not body:
        sys.exit("adapter.rs no longer declares PLAN_USAGE_ENDPOINTS -- update this parser")
    return dict(
        re.findall(r'provider_id: "([\w-]+)",\s*url: "([^"]+)"', body.group(1))
    )


def parse_oauth(source: str) -> dict[str, str]:
    """`oauth_provider`: the providers whose grants Brama can refresh, and where."""
    body = re.search(r"fn oauth_provider\(provider: &str\).*?\n\}", source, re.S)
    if not body:
        sys.exit("oauth_refresh.rs no longer has `oauth_provider` -- update this parser")
    return dict(
        re.findall(r'"([\w-]+)" => Some\(OAuthProvider \{\s*token_endpoint: "([^"]+)"', body.group(0))
    )


def override_variable(provider_id: str) -> str:
    """The exact override name `provider_base_url_override` builds."""
    return "BRAMA_PROVIDER_" + "".join(
        c.upper() if c.isalnum() else "_" for c in provider_id
    ) + "_BASE_URL"


def chrome() -> tuple[str, str]:
    """The published page shell, with the sidebar's active markers stripped."""
    page = TEMPLATE_PAGE.read_text(encoding="utf-8")
    head, _, tail = page.partition("<main>")
    if not tail:
        sys.exit(f"{TEMPLATE_PAGE} has no <main> -- update this generator")
    _, _, foot = tail.partition("</main>")
    head = head.replace(' class="active" aria-current="page"', "")
    return head, "</main>" + foot


def retitle(head: str, title: str, description: str, canonical: str, active: str) -> str:
    head = re.sub(r"<title>.*?</title>", f"<title>{title} — Brama Docs</title>", head, count=1)
    head = re.sub(
        r'<meta name="description" content="[^"]*">',
        f'<meta name="description" content="{html.escape(description, quote=True)}">',
        head,
        count=1,
    )
    head = re.sub(
        r'<link rel="canonical" href="[^"]*">',
        f'<link rel="canonical" href="https://brama.wisent.com{canonical}">',
        head,
        count=1,
    )
    return head.replace(
        f'<a href="{active}">',
        f'<a class="active" aria-current="page" href="{active}">',
    )


def heading(level: int, anchor: str, text: str) -> str:
    return (
        f'<h{level} id="{anchor}">{text}'
        f'<a class="headerlink" href="#{anchor}" title="Link to this section">&para;</a>'
        f"</h{level}>"
    )


SUBSCRIPTION_IDS = ("claude-code", "codex", "kimi")


def provider_page(
    provider: dict,
    hosts: dict[str, list[str]],
    usage: dict[str, str],
    oauth: dict[str, str],
) -> str:
    pid = provider["id"]
    name = provider["display_name"]
    base = provider["base_url"]
    local = pid == "local-openai"
    subscription = pid in SUBSCRIPTION_IDS
    endpoint = lambda path: base.rstrip("/") + path  # noqa: E731 -- mirrors `endpoint` in adapter.rs

    parts = [heading(1, pid, f"{html.escape(name)} <code>{pid}</code>")]

    if subscription:
        ownership = (
            'a <a href="/docs/concepts/subscription">subscription</a> provider: its '
            "credentials are agent-owned OAuth grants from the pool, never the "
            "deployment&#x27;s own spend"
        )
    elif local:
        ownership = (
            "the deployment-owned inference target: its endpoint resolves per request "
            'through the routes file (<a href="/docs/concepts/alias">alias</a>), and only '
            "loopback or Tailscale IPv4 endpoints are accepted"
        )
    else:
        ownership = (
            'a direct provider: requests spend through the deployment&#x27;s own '
            '<a href="/docs/concepts/capability">capability</a> credential'
        )
    parts.append(
        f"<p>A protocol adapter (<a href=\"/docs/concepts/provider\">provider</a>) reached as "
        f"<code>{pid}/&lt;model&gt;</code>, speaking <code>{provider['wire']}</code>. "
        f"It is {ownership}. The descriptor says what the gateway can speak; a catalog entry "
        f"never unlocks a credential or manufactures availability.</p>"
    )

    parts.append(heading(2, "endpoints", "Endpoints and auth"))
    rows = [
        ("Base URL", f"<code>{base}</code>" + (" (per request, via the routes file)" if local else "")),
        ("Chat endpoint", f"<code>{endpoint(provider['chat_path'])}</code>"),
        ("Model discovery", f"<code>{endpoint(provider['models_path'])}</code>"),
        ("Wire protocol", f"<code>{provider['wire']}</code>"),
        ("Auth header", provider["auth"]),
        ("Base URL override", f"<code>{override_variable(pid)}</code>"),
    ]
    trusted = hosts.get(pid)
    if trusted:
        rows.insert(1, ("Trusted host", ", ".join(f"<code>{h}</code>" for h in trusted)))
    elif local:
        rows.insert(1, ("Trusted host", "none — loopback or Tailscale IPv4 only, via the routes file"))
    parts.append(
        "<table><thead><tr><th></th><th></th></tr></thead><tbody>"
        + "".join(f"<tr><td>{key}</td><td>{value}</td></tr>" for key, value in rows)
        + "</tbody></table>"
    )
    parts.append(
        "<p>Provider clients require approved HTTPS hosts, disable redirects, and bypass "
        "ambient proxies. The override is validated with the exact refusals listed in "
        '<a href="/docs/concepts/provider#endpoint-trust-and-overrides">provider</a>; a '
        "non-loopback override outside the trusted host above is refused as "
        "<code>host &lt;host&gt; is not trusted</code>.</p>"
    )

    parts.append(heading(2, "models", "Models"))
    if provider["static_models"]:
        parts.append(
            "<p>Pinned catalog rows (live discovery through the models endpoint widens, "
            "never replaces, these):</p><ul>"
            + "".join(f"<li><code>{pid}/{m}</code></li>" for m in provider["static_models"])
            + "</ul>"
        )
    else:
        parts.append(
            "<p>No pinned rows: the catalog is discovered live from the provider&#x27;s own "
            "models endpoint, bounded at 20 seconds per discovery. Public metadata comes from "
            "models.dev and is advisory only.</p>"
        )
    if pid == "openai":
        parts.append(
            "<p>Convenience routes resolve to pinned concrete models: "
            "<code>openai/default</code> → <code>gpt-5.4</code>, "
            "<code>openai/embeddings</code> → <code>text-embedding-3-small</code>, "
            "<code>openai/moderation</code> → <code>omni-moderation-latest</code>.</p>"
        )
    if pid == "qwen":
        parts.append(
            "<p>The convenience route <code>qwen/default</code> resolves to "
            "<code>qwen-max</code>.</p>"
        )

    if subscription:
        parts.append(heading(2, "credential-lifecycle", "Credential lifecycle"))
        lifecycle = [
            "<p>Credentials are OAuth grants held in the vault at "
            f"<code>provider:{pid}:&lt;subscription&gt;</code> and refreshed ahead of expiry "
        ]
        token_endpoint = oauth.get(pid)
        if token_endpoint:
            lifecycle.append(
                f"against <code>{token_endpoint}</code> "
                '(<a href="/docs/concepts/subscription#credential-lifecycle">subscription</a>). '
            )
        lifecycle.append(
            "A grant the provider disowned is only replaced by a real sign-in: "
            f"<code>brama subscription sign-in {pid} --reason &quot;&lt;why&gt;&quot;</code> "
            "drives one through Weles and proves it by the refresh that follows, and "
            f"<code>brama subscription refresh {pid} --reason &quot;&lt;why&gt;&quot;</code> "
            "forces the refresh alone "
            '(<a href="/docs/cli#brama-subscription-sign-in-provider-reason-text-json">cli</a>).</p>'
        )
        parts.append("".join(lifecycle))
        report = usage.get(pid)
        if report:
            parts.append(
                f"<p>This provider publishes a free plan-usage report at <code>{report}</code>, "
                "issued to exactly the OAuth credential the chat route already presents; the "
                "ledger records it per window "
                '(<a href="/docs/concepts/subscription#the-usage-ledger">subscription</a>).</p>'
            )
    elif not local:
        parts.append(heading(2, "credential", "Credential"))
        if pid == "anthropic":
            absence = (
                "The OAuth usage report at <code>api.anthropic.com/api/oauth/usage</code> is "
                "issued to OAuth credentials only, so this API-key route publishes no plan "
                "state — that absence is recorded as the provider&#x27;s own answer, never as "
                "a zero "
                '(<a href="/docs/concepts/subscription#the-usage-ledger">subscription</a>).'
            )
        else:
            absence = (
                "No plan-usage report is published — the absence is recorded as the "
                "provider&#x27;s own answer, never as a zero "
                '(<a href="/docs/concepts/subscription#the-usage-ledger">subscription</a>).'
            )
        parts.append(
            "<p>An API key redeemed at final use through the deployment&#x27;s "
            '<a href="/docs/concepts/capability">capability</a>; it has no refresh path and no '
            f"invented expiry. {absence}</p>"
        )
    else:
        parts.append(heading(2, "credential", "Credential"))
        parts.append(
            "<p>Brama does not start or supervise a local inference engine — the deployment "
            "owner controls the digest-pinned vLLM lifecycle. The bearer is whatever the "
            "routes file names for the resolved endpoint.</p>"
        )

    if pid == "codex":
        parts.append(
            "<p>Requests to the ChatGPT-account backend carry extra Codex headers beside the "
            "bearer (<code>authorize_provider</code> in <code>src/providers/adapter.rs</code>); "
            "every other provider&#x27;s requests stay byte-identical to the descriptor.</p>"
        )

    return "\n".join(parts)


def index_page(providers: list[dict], usage: dict[str, str], oauth: dict[str, str]) -> str:
    parts = [heading(1, "providers", "Providers")]
    parts.append(
        "<p>One page per provider descriptor, generated from "
        "<code>src/providers/adapter.rs</code> by "
        "<code>scripts/generate-provider-docs.py</code> — the table in the source is the "
        "truth and these pages follow it. What a provider is, and what it deliberately is "
        'not, is defined once in <a href="/docs/concepts/provider">concepts/provider</a>.</p>'
    )
    parts.append(
        "<table><thead><tr><th>id</th><th>Display name</th><th>Wire protocol</th>"
        "<th>Credentials</th><th>Plan usage</th></tr></thead><tbody>"
    )
    for provider in providers:
        pid = provider["id"]
        if pid in SUBSCRIPTION_IDS:
            credential = "subscription (OAuth)" + (" + refresh" if pid in oauth else "")
        elif pid == "local-openai":
            credential = "routes file"
        else:
            credential = "capability (API key)"
        parts.append(
            f'<tr><td><a href="/docs/providers/{pid}"><code>{pid}</code></a></td>'
            f"<td>{html.escape(provider['display_name'])}</td>"
            f"<td><code>{provider['wire']}</code></td>"
            f"<td>{credential}</td>"
            f"<td>{'yes' if pid in usage else '—'}</td></tr>"
        )
    parts.append("</tbody></table>")
    parts.append(
        "<p>Exactly three providers publish a free plan-usage report; the absence everywhere "
        "else is recorded as the provider&#x27;s own answer "
        '(<a href="/docs/concepts/subscription#the-usage-ledger">subscription</a>).</p>'
    )
    return "\n".join(parts)


def write_page(path: pathlib.Path, head: str, foot: str, title: str, description: str,
               canonical: str, body: str) -> None:
    page_head = retitle(head, title, description, canonical, "/docs/providers")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(page_head + "<main>\n" + body + "\n" + foot, encoding="utf-8")


def main() -> None:
    adapter = ADAPTER.read_text(encoding="utf-8")
    providers = parse_providers(adapter)
    hosts = parse_trusted_hosts(adapter)
    usage = parse_plan_usage(adapter)
    oauth = parse_oauth(OAUTH.read_text(encoding="utf-8"))
    head, foot = chrome()

    write_page(
        DOCS / "providers.html",
        head,
        foot,
        "Providers",
        "Every provider descriptor Brama speaks, one page each: endpoints, wire protocol, "
        "auth shape, trusted hosts, credentials and plan-usage reports.",
        "/docs/providers",
        index_page(providers, usage, oauth),
    )
    for provider in providers:
        write_page(
            DOCS / "providers" / f"{provider['id']}.html",
            head,
            foot,
            html.escape(provider["display_name"]),
            f"How Brama speaks to {provider['display_name']}: endpoints, wire protocol, "
            "auth shape, trusted host, override, and credential lifecycle.",
            f"/docs/providers/{provider['id']}",
            provider_page(provider, hosts, usage, oauth),
        )
    print(f"rendered {len(providers)} provider pages and the index under {DOCS}")


if __name__ == "__main__":
    main()
