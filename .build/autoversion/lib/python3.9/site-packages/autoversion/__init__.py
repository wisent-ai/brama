"""AutoVersion: one versioning rule, several implementations, one contract.

The rule answers a single question: given the version currently published, the
public surface that was published, and the surface being proposed — what kind of
change is this, and what is the next version?

It holds no knowledge of release channels, credentials, build systems, or any
product's layout. Surface extraction belongs to the product.
"""

from autoversion.rule import (
    ADDITIVE,
    BREAKING,
    INTERNAL,
    EmptySurface,
    NotATriple,
    NotCanonical,
    NotNumeric,
    RuleError,
    Version,
    classify,
    decide,
    is_canonical,
    version_newer,
    version_tuple,
)

__all__ = [
    "ADDITIVE",
    "BREAKING",
    "INTERNAL",
    "EmptySurface",
    "NotATriple",
    "NotCanonical",
    "NotNumeric",
    "RuleError",
    "Version",
    "classify",
    "decide",
    "is_canonical",
    "version_newer",
    "version_tuple",
]
