#!/usr/bin/env python3
"""Refuse a build whose router cannot answer the launcher.

`start-with-skarbiec` drives the entitlements router by name, and the router
is built from a different repository at a revision this one pins. Nothing
connected the two: the pin sat on a revision from before the capability broker
existed while the launcher called `capability-issue` on every start, so every
release built for weeks produced a bundle that could not start, and the only
symptom on the host was a gateway that answered /health and served nothing.

This reads the verbs the launcher invokes out of the launcher itself, asks the
built router about each one, and fails naming the difference. The launcher is a
family of files - an entry point and the stages it sources - and every one of
them is named here, because the verbs live in the stages and reading the entry
point alone would find none.

The question is put by invoking the verb, not by reading `--help`: the
capability verbs are dispatched ahead of the documented command table, so help
omits them and a help-based check answers no for a router that has them. A
router without the verb says `unknown command`. The vault is pointed at a path
that does not exist, so nothing real is touched either way.

    check-router-verbs.py <router-binary> <launcher> [<launcher-stage>...]
"""

import os
import re
import subprocess
import sys

SHELL_CALL = re.compile(r'"\$ENTITLEMENTS_ROUTER_BIN"\s+([a-z][a-z-]*)')
PYTHON_CALL = re.compile(r'^\s*router,\s*$\n\s*"([a-z][a-z-]*)"', re.MULTILINE)
REFUSAL = "unknown command"

arguments = list(sys.argv)[1:]
if len(arguments) < 2:
    raise SystemExit(
        "usage: check-router-verbs.py <router-binary> <launcher> [<launcher-stage>...]"
    )
router_path = arguments[0]
launcher_paths = arguments[1:]

launcher = "\n".join(
    open(launcher_path, encoding="utf-8").read() for launcher_path in launcher_paths
)
required = sorted(set(SHELL_CALL.findall(launcher)) | set(PYTHON_CALL.findall(launcher)))
if not required:
    raise SystemExit(
        f"{', '.join(launcher_paths)} invokes no router verb this check can see; "
        "the patterns and the launcher have drifted apart"
    )

environment = dict(os.environ)
environment["SKARBIEC_VAULT_FILE"] = os.path.join(
    os.path.dirname(os.path.abspath(router_path)), "no-such-vault.json"
)

print(f"launcher requires: {', '.join(required)}")
missing = []
for verb in required:
    answered = subprocess.run(
        [router_path, verb],
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if REFUSAL in (answered.stdout + answered.stderr):
        missing.append(verb)

if missing:
    raise SystemExit(
        f"the pinned Skarbiec build does not implement {', '.join(missing)}; bump "
        "SKARBIEC_RELEASE_REVISION to a revision that does, or stop calling it"
    )
print("the pinned Skarbiec build answers every verb the launcher calls")
