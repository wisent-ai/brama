"""Surface extractors for language conventions, not for products.

The rule takes a set of public names and does not care where they came from. Most
products can therefore keep their own extractor. But a few conventions are shared by
every project in a language — `__all__` in a Python package is the obvious one — and
leaving those to each repository means every repository writes the same AST walk,
which is the duplication this project exists to end.

The line is deliberate and narrow:

- **In scope:** a convention defined by the language or its packaging, identical
  across every project that uses it.
- **Out of scope:** where a product keeps its files, what it calls its entry point,
  which of its modules are public. That is product knowledge and belongs to the
  product.
"""

from __future__ import annotations

import ast
import pathlib


class SurfaceError(Exception):
    """The surface could not be read from where the caller pointed."""


def python_all(path: str | pathlib.Path) -> list:
    """The names in a Python module's `__all__`.

    Read with `ast` rather than by importing, because importing a package to learn
    its public names runs its side effects and requires its dependencies — neither
    of which a release decision should need.
    """
    source = pathlib.Path(path)
    try:
        tree = ast.parse(source.read_text(), filename=str(source))
    except OSError as error:
        raise SurfaceError(f"{source}: {error}") from error
    except SyntaxError as error:
        raise SurfaceError(f"{source}: not parseable Python: {error}") from error

    for node in ast.walk(tree):
        if not isinstance(node, ast.Assign):
            continue
        for target in node.targets:
            if isinstance(target, ast.Name) and target.id == "__all__":
                if not isinstance(node.value, (ast.List, ast.Tuple, ast.Set)):
                    raise SurfaceError(
                        f"{source}: __all__ is not a literal list, tuple or set, so "
                        "its contents cannot be read without executing the module"
                    )
                names = []
                for element in node.value.elts:
                    if not isinstance(element, ast.Constant) or not isinstance(
                        element.value, str
                    ):
                        raise SurfaceError(
                            f"{source}: __all__ contains an entry that is not a "
                            "string literal"
                        )
                    names.append(element.value)
                return names

    raise SurfaceError(
        f"{source}: no __all__ found. A package without one has no declared public "
        "surface, so a release cannot be classified from it — declare __all__, or "
        "supply a surface the product extracts itself"
    )


def python_declared(path: str | pathlib.Path) -> list:
    """Public names a module binds, for packages that have not declared `__all__`.

    This mirrors what Python itself exposes to `from module import *` when `__all__`
    is absent: module-level names that do not begin with an underscore. Two
    departures, both to keep the result meaningful as a *contract*:

    - a plain `import os` binds `os` at module level, and Python would export it, but
      nobody promised it. Those bindings are skipped, so rearranging imports does not
      read as an added or removed promise.
    - `from x import y` is kept, because re-exporting is how packages publish names
      they did not define.

    This is a transitional measure. A package that declares `__all__` states its
    contract deliberately, which is what the release policy asks for; inferring it
    means the contract changes whenever someone renames a local helper.
    """
    source = pathlib.Path(path)
    try:
        tree = ast.parse(source.read_text(), filename=str(source))
    except OSError as error:
        raise SurfaceError(f"{source}: {error}") from error
    except SyntaxError as error:
        raise SurfaceError(f"{source}: not parseable Python: {error}") from error

    names = []

    def keep(name: str) -> None:
        if not name.startswith("_") and name not in names:
            names.append(name)

    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            keep(node.name)
        elif isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    keep(target.id)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            keep(node.target.id)
        elif isinstance(node, ast.ImportFrom):
            for alias in node.names:
                if alias.name != "*":
                    keep(alias.asname or alias.name)

    if not names:
        raise SurfaceError(
            f"{source}: no public names found, with or without __all__"
        )
    return names


def python_surface(path: str | pathlib.Path, allow_fallback: bool = False) -> tuple:
    """The declared surface, or the inferred one when the caller permits it.

    Returns the names and whether they were inferred, so a caller can say so rather
    than presenting a guess as a declaration.
    """
    try:
        return python_all(path), False
    except SurfaceError:
        if not allow_fallback:
            raise
        return python_declared(path), True
