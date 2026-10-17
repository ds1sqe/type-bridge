"""Value-free provider outcomes shared by ordinary transaction errors."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Failure:
    """A proven rejection or an indeterminate commit; values are never included."""

    phase: str
    reason: str | None
    lock_kind: str | None
    annotation: str | None
    op_index: int | None
    retry_safe: bool
    code: str | None


class CommitRejected(RuntimeError):  # noqa: N818 - shared SDK outcome name
    """The provider proves that the transaction did not commit."""

    failure: Failure


class CommitConflict(CommitRejected):
    """A concurrent commit conflicted with this transaction."""


class CommitOutcomeUnknown(RuntimeError):  # noqa: N818 - shared SDK outcome name
    """The commit may have applied; reconcile state before retrying."""

    failure: Failure
