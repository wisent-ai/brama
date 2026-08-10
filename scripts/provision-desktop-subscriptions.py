#!/usr/bin/env python3
"""Materialize the four Brama Desktop subscription items without printing secrets.

Discovery contract: a subscription item is any vault bundle tagged
`brama:subscription` plus `brama:agent:<agent>` for every owning agent. The
provider and the subscription id travel in `brama:provider:<name>` and
`brama:id:<name>` tags, so the item id itself is opaque and renames break
nothing.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = Path(os.environ.get("SKARBIEC_BIN", HOME / ".stado" / "bin" / "skarbiec"))
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", HOME / ".stado" / "skarbiec.vault.json")
)
RECIPIENTS = (
    "skarbiec-owner-charless-mini-20260804",
    "skarbiec-owner-20260728 <lukaszbartoszcze@wisent.ai>",
    "brama-rtx@wisent.local",
)
CATALOG = Path(__file__).with_name("skarbiec-subscriptions.json")
SOURCES = {
    "brama-sub-wisent-app-codex-primary": "codex-reauth-config",
    "brama-sub-wisent-app-codex-secondary": "codex-reauth-config",
    "brama-sub-wisent-app-claude-primary": "claude-reauth-config",
    "brama-sub-wisent-app-kimi-primary": "kimi-reauth-config",
}
ENVIRONMENT = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "SKARBIEC_VAULT_FILE": str(VAULT),
}


def read_item(identifier: str) -> dict[str, object]:
    opened = subprocess.run(
        [str(SKARBIEC), "get", identifier],
        capture_output=True,
        text=True,
        env=ENVIRONMENT,
        check=False,
    )
    if opened.returncode:
        raise SystemExit(f"cannot read required source item {identifier}: {opened.stderr.strip()}")
    document = json.loads(opened.stdout)
    fields = document.get("fields")
    if not isinstance(fields, dict) or set(fields) != {"value"} or not fields["value"]:
        raise SystemExit(f"required source item {identifier} has an incompatible field shape")
    return document


def write_subscription(item_id: str, entry: dict[str, object], source: str) -> None:
    document = read_item(source)
    document["kind"] = "bundle"
    context = document.get("context")
    if not isinstance(context, dict):
        context = {}
        document["context"] = context
    context["source_item"] = source
    context["subscription_owner"] = "wisent-app"
    subscription_id = str(entry["id"])
    agents = [str(agent) for agent in entry.get("agents", ["wisent-app"])]
    tags = [
        "brama:subscription",
        f"brama:provider:{entry['provider']}",
        f"brama:id:{subscription_id}",
        *[f"brama:agent:{agent}" for agent in agents],
    ]
    written = subprocess.run(
        [
            str(SKARBIEC),
            "set-json",
            item_id,
            "--recipients",
            ",".join(RECIPIENTS),
            "--tags",
            ",".join(tags),
        ],
        input=json.dumps(document),
        capture_output=True,
        text=True,
        env=ENVIRONMENT,
        check=False,
    )
    if written.returncode:
        raise SystemExit(f"cannot write subscription item {item_id}: {written.stderr.strip()}")
    print(item_id)


catalog = json.loads(CATALOG.read_text())
for row in catalog:
    subscription_id = row["id"]
    source = SOURCES.get(subscription_id)
    if source is None:
        raise SystemExit(f"catalog entry {subscription_id} has no source credential mapping")
    write_subscription(f"provider:{row['provider']}:{subscription_id}", row, source)
