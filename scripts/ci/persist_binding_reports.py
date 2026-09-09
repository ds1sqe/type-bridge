"""Create-new publication for one validated Python/Node/Rust/C report set."""

from __future__ import annotations

import os
import shutil
import stat
import tempfile
from pathlib import Path, PurePosixPath

BINDINGS = ("python", "node", "rust", "c")


class PublishError(RuntimeError):
    """A validated report set could not be published without ambiguity."""


def publish(reports: tuple[Path, ...], output: Path, *, checkout: Path) -> None:
    if len(reports) != len(BINDINGS):
        raise PublishError("report set must contain exactly four paths")
    publish_files(
        {f"{binding}.json": path for binding, path in zip(BINDINGS, reports, strict=True)},
        output,
        checkout=checkout,
    )


def publish_files(reports: dict[str, Path], output: Path, *, checkout: Path) -> None:
    """Publish flat or grouped validated reports as one new directory."""
    output = output.resolve()
    if output.exists() or output == checkout or checkout in output.parents:
        raise PublishError("output must be a new directory outside the checkout")
    try:
        parent = output.parent.lstat()
    except OSError as error:
        raise PublishError("output parent cannot be inspected") from error
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise PublishError("output parent must be a real directory")
    if not reports:
        raise PublishError("report set must not be empty")
    for binding, report in reports.items():
        relative = PurePosixPath(binding)
        if (
            relative.is_absolute()
            or relative.as_posix() != binding
            or any(part in {".", ".."} for part in relative.parts)
            or "\\" in binding
            or relative.suffix != ".json"
        ):
            raise PublishError("report name must be a relative JSON path")
        try:
            metadata = report.lstat()
        except OSError as error:
            raise PublishError(f"{binding} report cannot be inspected") from error
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise PublishError(f"{binding} report must be a regular non-symlink file")

    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        for name, report in reports.items():
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(report, target)
        stage.rename(output)
    except OSError as error:
        raise PublishError("validated reports could not be persisted") from error
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def publish_bytes(
    path: Path,
    raw: bytes,
    error_type: type[Exception],
    *,
    maximum: int | None = None,
) -> None:
    """Durably create one evidence file without replacing an existing destination."""
    if not path.is_absolute():
        raise error_type("invalid_output_path", "output path must be absolute")
    try:
        parent = path.parent.lstat()
    except OSError as error:
        raise error_type("invalid_output_path", "output parent cannot be inspected") from error
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise error_type("invalid_output_path", "output parent must be a real directory")
    if maximum is not None and len(raw) > maximum:
        raise error_type("report_size_limit", "assembled report exceeds its size limit")
    temporary: Path | None = None
    try:
        descriptor, temporary_name = tempfile.mkstemp(
            prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
        )
        temporary = Path(temporary_name)
        with os.fdopen(descriptor, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.link(temporary, path, follow_symlinks=False)
    except OSError as error:
        raise error_type("report_publication_failed", "evidence cannot be created") from error
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
