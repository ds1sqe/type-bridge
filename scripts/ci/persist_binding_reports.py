"""Create-new publication for one validated Python/Node/Rust/C report set."""

from __future__ import annotations

import shutil
import stat
import tempfile
from pathlib import Path

BINDINGS = ("python", "node", "rust", "c")


class PublishError(RuntimeError):
    """A validated report set could not be published without ambiguity."""


def publish(reports: tuple[Path, ...], output: Path, *, checkout: Path) -> None:
    output = output.resolve()
    if output.exists() or output == checkout or checkout in output.parents:
        raise PublishError("output must be a new directory outside the checkout")
    try:
        parent = output.parent.lstat()
    except OSError as error:
        raise PublishError("output parent cannot be inspected") from error
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise PublishError("output parent must be a real directory")
    if len(reports) != len(BINDINGS):
        raise PublishError("report set must contain exactly four paths")
    for binding, report in zip(BINDINGS, reports, strict=True):
        try:
            metadata = report.lstat()
        except OSError as error:
            raise PublishError(f"{binding} report cannot be inspected") from error
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise PublishError(f"{binding} report must be a regular non-symlink file")

    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        for binding, report in zip(BINDINGS, reports, strict=True):
            shutil.copyfile(report, stage / f"{binding}.json")
        stage.rename(output)
    except OSError as error:
        raise PublishError("validated reports could not be persisted") from error
    finally:
        if stage.exists():
            shutil.rmtree(stage)
