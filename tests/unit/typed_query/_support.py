"""Internal construction helpers for offline typed-query unit tests."""

from tests.unit.typed_query._corpus import corpus_error as corpus_error
from type_bridge.typed import QuerySession


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
