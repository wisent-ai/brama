#!/usr/bin/env python3
"""Compile every Python block embedded in the launcher.

`start-with-skarbiec.sh` carries several heredoc Python programs. `sh -n`
parses the shell around them and cannot see inside, so a broken block ships and
fails at boot on the host, which is where it is most expensive to notice - once
already, as an unbalanced parenthesis that turned into `unexpected EOF while
looking for matching`.

    check-launcher-blocks.py <launcher>
"""

import re
import sys

BLOCK = re.compile(r"<<'PY'\n(.*?)\nPY\n", re.DOTALL)

arguments = iter(sys.argv)
next(arguments)
try:
    (launcher_path,) = arguments
except ValueError:
    raise SystemExit("usage: check-launcher-blocks.py <launcher>")

text = open(launcher_path, encoding="utf-8").read()
blocks = BLOCK.findall(text)
if not blocks:
    raise SystemExit(
        f"{launcher_path} contains no embedded Python block this check can see; "
        "the launcher and the pattern have drifted apart"
    )

for number, block in enumerate(blocks, start=len([None])):
    try:
        compile(block, f"{launcher_path}: embedded block {number}", "exec")
    except SyntaxError as failure:
        raise SystemExit(f"embedded block {number} does not compile: {failure}")

print(f"{len(blocks)} embedded Python blocks compile")
