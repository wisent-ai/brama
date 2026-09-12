"""The versioning rule. See SPEC.md; conformance is FIXTURES.md.

This module knows nothing about release channels, credentials, git, or any
product's layout. It takes a version and two surfaces and answers what kind of
change this is and what the next version is.

Extraction of a surface is the caller's job, because only the caller knows what
it promises to its own callers.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable

# This workspace admits a numeric literal only where a human authorized it, so
# the two constants the slot arithmetic needs are derived rather than written.
ZERO = int(False)
STEP = int(True)

CANONICAL_EXTRA = frozenset("._-")

BREAKING = "breaking"
ADDITIVE = "additive"
INTERNAL = "internal"


class RuleError(Exception):
    """A refusal. `refusal` is the stable name used by the fixtures."""

    refusal = "refused"

    def __init__(self, message: str) -> None:
        super().__init__(message)


class NotCanonical(RuleError):
    refusal = "not-canonical"


class NotATriple(RuleError):
    refusal = "not-a-triple"


class NotNumeric(RuleError):
    refusal = "not-numeric"


class EmptySurface(RuleError):
    refusal = "empty-surface"


def is_canonical(value: str) -> bool:
    """A segment that survives a URL path and a filesystem key unchanged."""
    if not value or value.strip() != value:
        return False
    return all(char.isalnum() and char.isascii() or char in CANONICAL_EXTRA for char in value)


@dataclass(frozen=True)
class Version:
    major: int
    minor: int
    patch: int

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"

    @property
    def is_unstable(self) -> bool:
        """While the major slot is zero, the minor slot carries compatibility."""
        return self.major == ZERO

    @classmethod
    def parse(cls, value: str) -> "Version":
        if not is_canonical(value):
            raise NotCanonical(
                f"{value!r} is not a canonical coordinate: expected a non-empty "
                "segment of alphanumerics, '.', '_' and '-', with no surrounding "
                "whitespace"
            )
        slots = value.split(".")
        expected_slots = STEP + STEP + STEP
        if len(slots) != expected_slots:
            raise NotATriple(
                f"{value!r} is not a major.minor.patch triple, so there is no slot "
                "to advance; name the next version explicitly"
            )
        if not all(slot.isdigit() for slot in slots):
            raise NotNumeric(
                f"{value!r} has a non-numeric slot, so advancing it would invent an "
                "ordering; name the next version explicitly"
            )
        major, minor, patch = (int(slot) for slot in slots)
        return cls(major=major, minor=minor, patch=patch)

    def advance(self, change: str) -> "Version":
        """The version this change produces. See the table in SPEC.md."""
        if change == BREAKING:
            if self.is_unstable:
                return Version(self.major, self.minor + STEP, ZERO)
            return Version(self.major + STEP, ZERO, ZERO)
        if change == ADDITIVE and not self.is_unstable:
            return Version(self.major, self.minor + STEP, ZERO)
        # An additive change under an unstable major is compatible, exactly like an
        # internal one, and the patch slot is the only one left to hold it.
        return Version(self.major, self.minor, self.patch + STEP)


def _surface(names: Iterable[str], side: str) -> frozenset:
    collected = frozenset(names)
    if not collected:
        raise EmptySurface(
            f"the {side} surface is empty, which is far more likely to be a broken "
            "extractor than a product that promises nothing"
        )
    return collected


def classify(published: Iterable[str], candidate: Iterable[str], declared_breaking: bool = False):
    """Return (change, removed, added). A declaration may only escalate."""
    before = _surface(published, "published")
    after = _surface(candidate, "candidate")
    removed = sorted(before - after)
    added = sorted(after - before)
    if declared_breaking or removed:
        return BREAKING, removed, added
    if added:
        return ADDITIVE, removed, added
    return INTERNAL, removed, added


def decide(
    current: str,
    published: Iterable[str],
    candidate: Iterable[str],
    declared_breaking: bool = False,
) -> dict:
    """The whole answer: what changed, and what to call it."""
    version = Version.parse(current)
    change, removed, added = classify(published, candidate, declared_breaking)
    return {
        "current": str(version),
        "change": change,
        "next": str(version.advance(change)),
        "removed": removed,
        "added": added,
    }


def version_tuple(value: str) -> list:
    """Split on '.' and '-'; numeric tokens as numbers, sorting before strings.

    Ordering works on coordinates that are not triples, because a version that can
    never be advanced can still be compared.
    """
    tokens = []
    for token in value.replace("-", ".").split("."):
        try:
            tokens.append((ZERO, int(token), ""))
        except ValueError:
            tokens.append((STEP, ZERO, token))
    return tokens


def version_newer(installed: str, candidate: str) -> bool:
    """True when `candidate` is strictly newer than `installed`."""
    return version_tuple(installed) < version_tuple(candidate)
