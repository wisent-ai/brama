#!/usr/bin/env python3
"""Device/runtime guard for hook modifications.

Blocks adding or editing hook sources/configs unless the user has explicitly
approved hook changes through DEVICE_HOOK_EDIT_APPROVED=1. This is intentionally
runtime-level, not project-level: it protects repo hooks, shared hooks, Codex,
OMP, and Claude hook/config locations.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import sys
from typing import Any, Iterable

APPROVAL_ENV = "DEVICE_HOOK_EDIT_APPROVED"
DOCS = (
    "hooks-rotator/docs/HOOK-MANAGEMENT.md",
    "hooks-rotator/docs/HOOKS.md",
    "hooks-rotator/skills/hook-management/SKILL.md",
)
WRITE_TOOLS = {"write", "edit", "multiedit", "apply_patch", "functions.apply_patch", "applypatch"}
SHELL_TOOLS = {"bash", "shell", "exec", "exec_command", "run_command", "runcommands", "terminal"}

PROTECTED_PATH_RE = re.compile(
    r"("
    r"(^|/)\.git/hooks(/|$)"
    r"|(^|/)\.githooks(/|$)"
    r"|(^|/)repo-githooks(/|$)"
    r"|(^|/)\.husky(/|$)"
    r"|(^|/)lefthook\.ya?ml$"
    r"|(^|/)\.pre-commit-config\.ya?ml$"
    r"|(^|/)\.claude/(settings[^/]*\.json|hooks(/|$))"
    r"|(^|/)\.codex/(hooks\.json|hooks(/|$))"
    r"|(^|/)\.omp/agent/hooks(/|$)"
    r"|(^|/)\.shared-hooks(/|$)"
    r"|(^|/)hooks-rotator/shared-hooks(/|$)"
    r"|(^|/)hooks-rotator/codex-hooks(/|$)"
    r"|(^|/)hooks-rotator/claude-hooks(/|$)"
    r"|(^|/)hooks-rotator/repo-githooks(/|$)"
    r")",
    re.IGNORECASE,
)
WRITE_COMMAND_RE = re.compile(
    r"("
    r">\s*[^\s;|&]+"
    r"|\b(tee|touch|mkdir|cp|mv|install|rsync|dd|chmod)\b"
    r"|\bsed\s+[^;&|]*-[^;&|]*i\b"
    r"|\b(writeFileSync|writeFile|open\s*\([^)]*['\"]w|Path\([^)]*\)\.write_text)\b"
    r")",
    re.IGNORECASE,
)

OPAQUE_PATCH_RE = re.compile(r"\b(git\s+apply|patch\b|ed\b|ex\b)", re.IGNORECASE)
GIT_HOOKSPATH_RE = re.compile(r"\bgit\s+config\b[^;&|\n]*\bcore\.hooksPath\b", re.IGNORECASE)


def load_payload() -> dict[str, Any]:
    raw = sys.stdin.read()
    try:
        data = json.loads(raw or "{}")
    except json.JSONDecodeError:
        data = {}
    return data if isinstance(data, dict) else {}


def tool_name(payload: dict[str, Any]) -> str:
    return str(payload.get("tool_name") or payload.get("tool") or payload.get("name") or "")


def tool_input(payload: dict[str, Any]) -> dict[str, Any]:
    value = payload.get("tool_input") or payload.get("input") or {}
    return value if isinstance(value, dict) else {}


def patch_candidate_paths(text: str) -> Iterable[str]:
    for raw_line in text.splitlines():
        line = raw_line.strip()
        bracket = re.match(r"^\[([^#\]\n]+)#", line)
        if bracket:
            yield bracket.group(1)
            continue
        file_header = re.match(r"^\*\*\* (?:Add|Update|Delete) File: (.+)$", line)
        if file_header:
            yield file_header.group(1).strip()
            continue
        diff_header = re.match(r"^(?:---|\+\+\+) [ab]/(.+)$", line)
        if diff_header:
            yield diff_header.group(1).strip()


def candidate_paths(value: Any) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key in ("file_path", "path", "notebook_path"):
            item = value.get(key)
            if isinstance(item, str):
                yield item
        for key in ("input", "patch"):
            item = value.get(key)
            if isinstance(item, str):
                yield from patch_candidate_paths(item)
        edits = value.get("edits")
        if isinstance(edits, list):
            for edit in edits:
                yield from candidate_paths(edit)
    elif isinstance(value, list):
        for item in value:
            yield from candidate_paths(item)


def protected_path(path: str) -> bool:
    return bool(PROTECTED_PATH_RE.search(path.replace("\\", "/")))


def mentions_git_hookspath(value: Any) -> bool:
    return "hookspath" in json.dumps(value, ensure_ascii=False).lower()


def git_config_path(path: str) -> bool:
    return bool(re.search(r"(^|/)\.git/config$", path.replace("\\", "/"), re.IGNORECASE))


def command_mentions_protected_path(command: str) -> bool:
    normalized = command.replace("\\", "/")
    if PROTECTED_PATH_RE.search(normalized):
        return True
    for token in re.split(r"[\s;&|<>()]+", normalized):
        stripped = token.strip("\"'[]{}:,")
        if stripped and protected_path(stripped):
            return True
    return False


def approved() -> bool:
    return os.environ.get(APPROVAL_ENV) == "1"


def command_text(payload: dict[str, Any]) -> str:
    inp = tool_input(payload)
    return str(inp.get("command") or inp.get("cmd") or payload.get("command") or "")


def git_configures_hookspath(command: str) -> bool:
    for segment in re.split(r"[;&|]+", command):
        try:
            tokens = shlex.split(segment)
        except ValueError:
            tokens = segment.split()
        for index, token in enumerate(tokens):
            if token != "git":
                continue
            cursor = index + 1
            while cursor < len(tokens) and tokens[cursor].startswith("-"):
                option = tokens[cursor]
                cursor += 1
                if option in {"-C", "--git-dir", "--work-tree", "-c"} and cursor < len(tokens):
                    cursor += 1
            if cursor < len(tokens) and tokens[cursor] == "config":
                if any(item.lower() == "core.hookspath" for item in tokens[cursor + 1:]):
                    return True
    return False


def bash_targets_hook_modification(payload: dict[str, Any]) -> bool:
    command = command_text(payload)
    if not command:
        return False
    normalized = command.replace("\\", "/")
    if OPAQUE_PATCH_RE.search(normalized) or GIT_HOOKSPATH_RE.search(normalized) or git_configures_hookspath(normalized):
        return True
    return bool(command_mentions_protected_path(normalized) and WRITE_COMMAND_RE.search(normalized))


def block_reason(targets: list[str]) -> str:
    docs = " | ".join(DOCS)
    return (
        "BLOCKED: hook source/config changes are device-level protected. "
        f"Set {APPROVAL_ENV}=1 outside the agent session before editing hooks. "
        f"Read hook scope docs: {docs}. "
        f"Target(s): {', '.join(targets[:5])}"
    )


def main() -> int:
    payload = load_payload()
    tool = tool_name(payload).lower()

    targets: list[str] = []
    if tool in WRITE_TOOLS:
        inp = tool_input(payload)
        paths = [p for p in candidate_paths(inp) if isinstance(p, str) and p]
        targets = [p for p in paths if protected_path(p) or (git_config_path(p) and mentions_git_hookspath(inp))]
        if not targets and tool in {"apply_patch", "functions.apply_patch", "applypatch"}:
            serialized = json.dumps(inp, ensure_ascii=False)
            if command_mentions_protected_path(serialized) or mentions_git_hookspath(inp):
                targets = ["apply_patch payload can create/edit hook source/config"]
    elif tool in SHELL_TOOLS:
        if bash_targets_hook_modification(payload):
            targets = ["Shell command can create/edit hook source/config"]
    else:
        return 0

    if not targets:
        return 0
    if approved():
        return 0

    print(block_reason(targets), file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
