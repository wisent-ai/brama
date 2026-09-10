#!/usr/bin/env python3
"""Compile every Python block embedded in the launcher family.

`start-with-skarbiec` and the stage files it sources carry several heredoc
Python programs. `sh -n` parses the shell around them and cannot see inside, so
a broken block ships and fails at boot on the host, which is where it is most
expensive to notice - once already, as an unbalanced parenthesis that turned
into `unexpected EOF while looking for matching`.

Every stage file is compiled, not only the entry point: the launcher was one
file when this check was written, and the blocks now live in the stages. The
family is derived from the entry point rather than listed by a caller, because
a hand-kept list is what drifted: CI named all eight stage files while the
release recipe still named only `start-with-skarbiec`, so the same source
passed one gate and failed the other with "no embedded Python block this check
can see". An argument list that yields no block at all is still refused, since
that means the pattern itself has moved.

    check-launcher-blocks.py <launcher> [<launcher-stage>...]
"""

import sys
from pathlib import Path


def family(arguments):
    """Every launcher file to compile: what was named, plus the stage files
    beside each named entry point. `sh` sources the stages, so they are part of
    the same program and share its blast radius."""
    named = [Path(argument) for argument in arguments]
    files = list(named)
    for path in named:
        stages = sorted((path.parent / "launcher").rglob("*.sh"))
        files.extend(stage for stage in stages if stage not in files)
        helpers = sorted((path.parent.parent / "libexec").rglob("*.py"))
        files.extend(helper for helper in helpers if helper not in files)
    return files


def read_text(path):
    """One launcher file as text, refusing a binary argument by name.

    The release recipe passed the router-verb check's arguments in the wrong
    order, the router binary landed where a launcher belongs, and Python
    answered with a codec traceback naming a byte offset -- which says nothing
    about which argument was wrong. A caller that hands over a binary is told
    so. `check_router_verbs.py` imports both of these, so the family is
    derived once for the whole release gate.
    """
    try:
        return Path(path).read_text(encoding="utf-8")
    except UnicodeDecodeError as failure:
        raise SystemExit(
            f"{path} is not a launcher: it is not UTF-8 text ({failure}). "
            "The launcher and binary arguments are in the wrong order"
        )

def python_blocks(path):
    """The Python units shipped directly or embedded in one launcher stage."""
    text = read_text(path)
    if Path(path).suffix == ".py":
        return [text]
    blocks = []
    current = None
    for line in text.splitlines():
        if current is None:
            if line.rstrip().endswith("<<'PY'"):
                current = []
        elif line == "PY":
            blocks.append("\n".join(current))
            current = None
        else:
            current.append(line)
    if current is not None:
        raise SystemExit(f"{path} contains an unterminated embedded Python block")
    return blocks


def main(arguments):
    if not arguments:
        raise SystemExit(
            "usage: check-launcher-blocks.py <launcher> [<launcher-stage>...]"
        )
    blocks = []
    for launcher_path in family(arguments):
        blocks.extend((launcher_path, block) for block in python_blocks(launcher_path))
    if not blocks:
        raise SystemExit(
            "the launcher family named here contains no embedded Python block this "
            "check can see; the launcher and the pattern have drifted apart: "
            + ", ".join(str(path) for path in family(arguments))
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


if __name__ == "__main__":
    main(list(sys.argv)[1:])
