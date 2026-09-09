#!/usr/bin/env python3
"""Compile every Python block embedded in the launcher family.

`start-with-skarbiec` and the stage files it sources carry several heredoc
Python programs. `sh -n` parses the shell around them and cannot see inside, so
a broken block ships and fails at boot on the host, which is where it is most
expensive to notice - once already, as an unbalanced parenthesis that turned
into `unexpected EOF while looking for matching`.

Every stage file is named, not only the entry point: the launcher was one file
when this check was written, and the blocks now live in the stages. A check
reading the entry point alone would compile nothing and say so, which is why an
argument list carrying no block at all is refused here.

    check-launcher-blocks.py <launcher> [<launcher-stage>...]
"""

import sys

arguments = list(sys.argv)[1:]
if not arguments:
    raise SystemExit("usage: check-launcher-blocks.py <launcher> [<launcher-stage>...]")

blocks = []
for launcher_path in arguments:
    lines = open(launcher_path, encoding="utf-8").read().splitlines()
    current = None
    for line in lines:
        if current is None:
            if line.endswith("<<'PY'"):
                current = []
            continue
        if line == "PY":
            blocks.append((launcher_path, "\n".join(current)))
            current = None
        else:
            current.append(line)
    if current is not None:
        raise SystemExit(f"{launcher_path} contains an unterminated embedded Python block")
if not blocks:
    raise SystemExit(
        "the launcher family named here contains no embedded Python block this "
        "check can see; the launcher and the pattern have drifted apart: "
        + ", ".join(arguments)
    )

for number, (launcher_path, block) in enumerate(blocks, start=len([None])):
    print(
        f"compiling embedded Python block {number} from {launcher_path}",
        file=sys.stderr,
        flush=True,
    )
    try:
        compile(block, f"{launcher_path}: embedded block {number}", "exec")
    except SyntaxError as failure:
        raise SystemExit(
            f"embedded block {number} in {launcher_path} does not compile: {failure}"
        )

print(f"{len(blocks)} embedded Python blocks compile")
