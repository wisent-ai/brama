#!/usr/bin/env python3
"""Check the launcher's literal command paths against the real broker inventory.

Only help inventories are executed. A group name alone cannot prove that the
subcommand the launcher invokes exists, and an operational leaf is not a probe.
"""
import ast
import json
import os
import re
import subprocess
import sys

from check_launcher_blocks import family, python_blocks, read_text

SHELL_CALL = re.compile(r'"\$ENTITLEMENTS_ROUTER_BIN"\s+([a-z][a-z-]*(?:\s+[a-z][a-z-]*)*)')


def inventory(binary, args, environment):
    result = subprocess.run([binary, *args], capture_output=True, text=True, env=environment, check=False)
    if result.returncode:
        raise SystemExit(f"broker inventory {' '.join(args)} failed: {result.stderr or result.stdout}")
    try:
        document = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"broker inventory {' '.join(args)} is not JSON: {error}") from error
    if not isinstance(document, dict) or not isinstance(document.get("commands"), list):
        raise SystemExit(f"broker inventory {' '.join(args)} has no commands")
    return document


def python_calls(text):
    for node in ast.walk(ast.parse(text)):
        if not isinstance(node, (ast.List, ast.Tuple)) or not node.elts:
            continue
        first, *arguments = node.elts
        if not isinstance(first, ast.Name) or first.id != "router":
            continue
        words = []
        for argument in arguments:
            if not isinstance(argument, ast.Constant) or not isinstance(argument.value, str):
                break
            if not re.fullmatch(r"[a-z][a-z-]*", argument.value):
                break
            words.append(argument.value)
        if words:
            yield words


def main():
    if len(sys.argv) < 3:
        raise SystemExit("usage: check_router_verbs.py <router-binary> <launcher> [<launcher-stage>...]")
    binary, *launchers = sys.argv[1:]
    environment = dict(os.environ)
    environment["SKARBIEC_VAULT_FILE"] = os.path.join(os.path.dirname(os.path.abspath(binary)), "no-such-vault.json")
    root = inventory(binary, ["help"], environment)
    groups = root.get("groups")
    if not isinstance(groups, list) or not all(isinstance(group, str) for group in groups):
        raise SystemExit("the pinned broker does not declare CLI groups; use Skarbiec 0.3.2 or newer")
    available = {tuple(command.split()[:1]) for command in root["commands"]}
    for group in groups:
        document = inventory(binary, [group, "help"], environment)
        available.update(tuple(command.split()[:2]) for command in document["commands"])
    required = set()
    for path in family(launchers):
        text = read_text(path)
        calls = [match.split() for match in SHELL_CALL.findall(text.replace("\\\n", " "))]
        for block in python_blocks(path):
            calls.extend(python_calls(block))
        for words in calls:
            required.add(tuple(words[:2] if words[0] in groups else words[:1]))
    if not required:
        raise SystemExit("the launcher has no literal router command this check can inspect")
    print("launcher requires: " + ", ".join(" ".join(command) for command in sorted(required)))
    missing = required - available
    if missing:
        raise SystemExit("the pinned broker does not implement: " + ", ".join(" ".join(command) for command in sorted(missing)))
    print("the pinned broker advertises every literal command path the launcher invokes")


if __name__ == "__main__":
    main()
