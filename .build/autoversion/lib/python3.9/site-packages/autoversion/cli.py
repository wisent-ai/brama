"""Command line for the rule, so a product in any language can ask it.

Python repositories import `autoversion` directly. Everything else — a Rust control
plane, a Swift app, a CI step — runs this. Either way there is one implementation of
the rule and no repository holds a copy of it.

Exit codes are the contract for automation: zero when the question was answered,
non-zero when it was refused. A refusal is not a failure of the caller's build; it
means the rule declined to invent something, and it always says what.
"""

from __future__ import annotations

import argparse
import json
import sys

from autoversion import rule, surfaces

OK = int(False)
REFUSED = int(True)


def _read_surface(path: str) -> list:
    with open(path) as handle:
        document = json.load(handle)
    names = document.get("surface")
    if names is None:
        raise rule.RuleError(
            f"{path}: no \"surface\" key. A surface document is "
            '{"surface": ["name", ...]}'
        )
    return names


def _emit(payload: dict, as_json: bool) -> None:
    if as_json:
        print(json.dumps(payload, indent=int(True) + int(True), sort_keys=True))
        return
    for key, value in payload.items():
        rendered = ", ".join(value) if isinstance(value, list) else value
        if rendered == "" or rendered == []:
            continue
        print(f"{key}: {rendered}")


def _decide(args: argparse.Namespace) -> int:
    published = _read_surface(args.published_surface)
    candidate = _read_surface(args.candidate_surface)
    answer = rule.decide(args.current, published, candidate, args.breaking)
    _emit(answer, args.json)
    return OK


def _check(args: argparse.Namespace) -> int:
    """The same decision, taking two Python packages instead of two surface files."""
    published, published_guessed = surfaces.python_surface(
        args.published_init, args.fallback
    )
    candidate, candidate_guessed = surfaces.python_surface(
        args.candidate_init, args.fallback
    )
    answer = rule.decide(args.current, published, candidate, args.breaking)
    if published_guessed or candidate_guessed:
        answer["surface"] = "inferred, because __all__ is absent"
    _emit(answer, args.json)
    if args.expect is not None and answer["next"] != args.expect:
        raise rule.RuleError(
            f"the surface change requires {answer['next']}, but the product declares "
            f"{args.expect}"
        )
    return OK


def _surface(args: argparse.Namespace) -> int:
    names, guessed = surfaces.python_surface(args.python_init, args.fallback)
    document = {"surface": names}
    if guessed:
        document["inferred"] = "no __all__; names were read from module-level bindings"
    print(json.dumps(document, indent=int(True) + int(True), sort_keys=True))
    return OK


def _order(args: argparse.Namespace) -> int:
    newer = rule.version_newer(args.older, args.newer)
    _emit({"older": args.older, "newer": args.newer, "is_newer": str(newer)}, args.json)
    return OK


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="autoversion",
        description="Decide a version change from two public surfaces.",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON")
    sub = parser.add_subparsers(dest="command", required=True)

    decide = sub.add_parser("decide", help="classify from two surface documents")
    decide.add_argument("--current", required=True, help="the published version")
    decide.add_argument("--published-surface", required=True)
    decide.add_argument("--candidate-surface", required=True)
    decide.add_argument(
        "--breaking",
        action="store_true",
        help="declare breakage the surface cannot show; escalates only",
    )
    decide.add_argument("--json", action="store_true")
    decide.set_defaults(handler=_decide)

    check = sub.add_parser(
        "check", help="classify two Python packages by their __all__, and verify a claim"
    )
    check.add_argument("--current", required=True, help="the published version")
    check.add_argument("--published-init", required=True, help="__init__.py as released")
    check.add_argument("--candidate-init", required=True, help="__init__.py proposed")
    check.add_argument(
        "--expect",
        help="the version the product declares; refuse when the rule disagrees",
    )
    check.add_argument("--breaking", action="store_true")
    check.add_argument(
        "--fallback",
        action="store_true",
        help="infer the surface from module-level bindings when __all__ is absent; "
        "transitional, and the result is labelled as inferred",
    )
    check.add_argument("--json", action="store_true")
    check.set_defaults(handler=_check)

    surface = sub.add_parser("surface", help="print a Python package's public surface")
    surface.add_argument("--python-init", required=True)
    surface.add_argument(
        "--fallback",
        action="store_true",
        help="infer the surface when __all__ is absent",
    )
    surface.add_argument("--json", action="store_true")
    surface.set_defaults(handler=_surface)

    order = sub.add_parser("order", help="ask which of two versions is newer")
    order.add_argument("--older", required=True)
    order.add_argument("--newer", required=True)
    order.add_argument("--json", action="store_true")
    order.set_defaults(handler=_order)

    return parser


def main(argv: list | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.handler(args)
    except (rule.RuleError, surfaces.SurfaceError) as error:
        print(f"refused: {error}", file=sys.stderr)
        return REFUSED


if __name__ == "__main__":
    sys.exit(main())
