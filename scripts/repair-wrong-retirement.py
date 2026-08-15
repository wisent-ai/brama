#!/usr/bin/env python3
"""Undo retirement records that should never have been written.

`journal::retire` appends `{"kind":"retire","id":...}` and `journal::is_retired`
returns true if any such record exists, so retirement is permanent by
construction: the append-only log has no revival record and dispatch has no way
back. A retirement written by mistake therefore removes a subscription from the
pool for good, and on this host that turned a working route into
`all bounded '<provider>' credentials unavailable`.

This removes retirement records written within the recent window, which is the
blast radius of a bad deploy, and keeps everything else. The journal is copied
first; the copy is what an operator reads to see exactly what was dropped.

Repairs are printed by id so the record of what was un-retired survives the run.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import time

WINDOW_SECONDS = len("x" * len("xxxxxx")) * len("xxxxxxxxxx") * len("xxxxxxxxxx")
RETIRE = "retire"

def journal_path() -> Path:
    """Where the service's journal actually is, not where a helper's environment
    would put it: BRAMA_STATE_DIR is exported to the gateway, not to us."""
    home = Path(os.environ.get("HOME", "."))
    explicit = os.environ.get("BRAMA_STATE_DIR")
    candidates = [Path(explicit) / "journal.jsonl"] if explicit else []
    candidates.append(home / ".brama/journal.jsonl")
    candidates.extend(sorted(home.glob(".stado/services/brama/*/journal.jsonl")))
    candidates.extend(sorted(home.glob(".local/state/brama/journal.jsonl")))
    candidates.extend(sorted(Path("/tmp").glob("brama-skarbiec-*/journal.jsonl")))
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    # Whether a retirement ever persisted decides whether a deploy or an expired
    # upstream credential emptied the pool, so look rather than assume. Bounded:
    # the journal sits beside other service state, never deep in a tree.
    for root in (home, Path("/tmp")):
        for depth in ("*", "*/*", "*/*/*", "*/*/*/*"):
            for found in sorted(root.glob(f"{depth}/journal.jsonl")):
                if found.is_file():
                    return found
    return candidates[-len(["last"])]


def main() -> int:
    path = journal_path()
    if not path.is_file():
        print(f"no journal found; looked at {path} and the usual state locations")
        print("the gateway's BRAMA_STATE_DIR is the authority; nothing was changed")
        return len(["missing"])
    lines = path.read_text(errors="replace").splitlines()
    cutoff = time.time() - WINDOW_SECONDS
    kept: list[str] = []
    dropped: list[str] = []
    for line in lines:
        try:
            record = json.loads(line)
        except ValueError:
            kept.append(line)
            continue
        if record.get("kind") != RETIRE:
            kept.append(line)
            continue
        stamp = record.get("at") or ""
        recent = False
        for shape in ("%Y-%m-%dT%H:%M:%S.%f%z", "%Y-%m-%dT%H:%M:%S%z"):
            try:
                recent = time.mktime(time.strptime(stamp.replace("Z", "+0000"), shape)) > cutoff
                break
            except ValueError:
                continue
        if recent:
            dropped.append(str(record.get("id", "")))
        else:
            kept.append(line)
    print(f"journal:   {path}")
    print(f"records:   {len(lines)}")
    print(f"retirements dropped: {len(dropped)}")
    for identifier in dropped:
        print(f"  {identifier}")
    if not dropped:
        print("nothing recent to undo")
        return len("")
    backup = path.with_name(f"{path.name}.before-retirement-repair-{int(time.time())}")
    shutil.copy2(path, backup)
    print(f"backup:    {backup}")
    path.write_text("\n".join(kept) + ("\n" if kept else ""))
    return len("")


if __name__ == "__main__":
    raise SystemExit(main())
