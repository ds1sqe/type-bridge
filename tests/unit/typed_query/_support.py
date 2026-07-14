"""Internal construction helpers for offline typed-query unit tests."""

import json
from functools import cache
from pathlib import Path

from type_bridge.typed import QuerySession

_CORPUS = Path(__file__).parents[2] / "contracts" / "typed_query" / "corpus-v1.json"


@cache
def _corpus_errors() -> dict[str, tuple[str, str]]:
    corpus = json.loads(_CORPUS.read_text(encoding="utf-8"))
    return {
        case["id"]: (case["expected"]["error_category"], case["expected"]["error_code"])
        for case in corpus["cases"]
        if case["expected"]["outcome"] == "error"
    }


def corpus_error(case_id: str) -> tuple[str, str]:
    """Return the shared #171 category/code pair for one public runtime case."""
    return _corpus_errors()[case_id]


def diagnostic_session() -> QuerySession:
    """Return a session that validates native plans without an execution target."""
    return QuerySession._diagnostic()


def invoke_untyped(function: object, /, *args: object, **kwargs: object) -> object:
    """Invoke a runtime boundary after proving the supplied value is callable.

    Hostile-input tests use this adapter to exercise dynamic Python calls
    without teaching consumer examples to suppress the static contract.
    """
    if not callable(function):
        raise TypeError("test boundary requires a callable value")
    return function(*args, **kwargs)


def runtime_attribute(owner: object, name: str) -> object:
    """Read a descriptor exactly as the Python runtime sees it."""
    return getattr(owner, name)
