"""Project Cargo.lock onto the immutable path dependencies supplied by Stado.

Only the selected packages' Git source lines change. Package versions, registry
checksums and dependency edges remain byte-for-byte the canonical lock's data;
cargo build --locked must still accept the resulting graph.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
from urllib.parse import urlsplit, urlunsplit


def prepare(source: Path, destination: Path, bindings: list[str]) -> None:
    paths = {}
    for binding in bindings:
        name, separator, directory = binding.partition("=")
        if not separator or not name or name in paths:
            raise ValueError(f"invalid or duplicate Cargo input: {binding}")
        path = Path(directory).resolve(strict=True)
        if not (path / "Cargo.toml").is_file():
            raise ValueError(f"Cargo input {name} has no manifest: {path}")
        paths[name] = path

    blocks = (source / "Cargo.lock").read_text().split("[[package]]\n")
    patches = []
    found = set()
    for index, block in enumerate(blocks):
        name_line = re.search(r'^name = ("[^"\n]+")$', block, re.MULTILINE)
        if name_line is None:
            continue
        name = json.loads(name_line.group(1))
        if name not in paths:
            continue
        if name in found:
            raise ValueError(f"Cargo.lock has several packages named {name}")
        source_line = re.search(r'^source = ("git\+[^"\n]+")\n', block, re.MULTILINE)
        if source_line is None:
            raise ValueError(f"Cargo.lock does not pin a Git source for {name}")
        coordinate = urlsplit(json.loads(source_line.group(1))[len("git+"):])
        origin = urlunsplit((coordinate.scheme, coordinate.netloc, coordinate.path, "", ""))
        patches.append(f'[patch.{json.dumps(origin)}]\n{name} = {{ path = {json.dumps(str(paths[name]))} }}\n')
        blocks[index] = block[:source_line.start()] + block[source_line.end():]
        found.add(name)
    missing = paths.keys() - found
    if missing:
        raise ValueError(f"Cargo.lock has no input packages: {', '.join(sorted(missing))}")

    shutil.copytree(source, destination, ignore=shutil.ignore_patterns(
        ".git", "target", "node_modules", ".wisent-output", ".build"))
    (destination / "Cargo.lock").write_text("[[package]]\n".join(blocks))
    (destination / "release-inputs.toml").write_text("\n".join(patches))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("binding", nargs="+", help="Cargo package=immutable source directory")
    args = parser.parse_args()
    prepare(args.source.resolve(strict=True), args.destination, args.binding)
