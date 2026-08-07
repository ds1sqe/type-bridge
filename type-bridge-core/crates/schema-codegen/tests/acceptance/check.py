from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[3]
ROOT = HERE.parents[4]
STAGE = CORE / "target" / "schema-codegen-python-acceptance"
DOCUMENTED_EXAMPLES = (
    ROOT / "tests" / "contracts" / "typed_query" / "python" / "documented_examples.py"
)
MARKER = re.compile(r"# E: (?P<marker>[a-z][a-z0-9_]*):(?P<rule>report[A-Za-z]+)$")


def command(
    arguments: list[str],
    *,
    expected: int = 0,
    cwd: Path = ROOT,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        arguments,
        cwd=cwd,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != expected:
        raise AssertionError(
            f"command returned {completed.returncode}, expected {expected}: {arguments}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def pyright(path: Path, *, expected_exit: int) -> dict[str, object]:
    completed = command(
        [
            "uv",
            "run",
            "pyright",
            "--outputjson",
            "--project",
            str(STAGE / "pyrightconfig.json"),
            str(path),
            str(STAGE / "generated_v2"),
        ],
        expected=expected_exit,
    )
    return json.loads(completed.stdout)


def expected_diagnostics(path: Path) -> dict[int, tuple[str, str]]:
    expected: dict[int, tuple[str, str]] = {}
    for line_number, line in enumerate(path.read_text().splitlines()):
        match = MARKER.search(line)
        if match is not None:
            expected[line_number] = (match["marker"], match["rule"])
    if not expected:
        raise AssertionError("negative fixture has no diagnostic markers")
    return expected


def check_negative(report: dict[str, object], path: Path) -> None:
    expected = expected_diagnostics(path)
    diagnostics = report["generalDiagnostics"]
    actual: dict[int, list[str]] = {}
    for diagnostic in diagnostics:
        if Path(diagnostic["file"]).resolve() != path.resolve():
            raise AssertionError(f"unexpected diagnostic outside negative fixture: {diagnostic}")
        line = diagnostic["range"]["start"]["line"]
        actual.setdefault(line, []).append(diagnostic.get("rule", ""))
    if set(actual) != set(expected):
        raise AssertionError(f"diagnostic lines differ: expected {expected}, actual {actual}")
    for line, (marker, rule) in expected.items():
        if actual[line] != [rule]:
            raise AssertionError(
                f"marker {marker} expected exactly {rule}, received {actual[line]}"
            )


def main() -> None:
    fixtures = [
        HERE / "positive.py",
        HERE / "negative.py",
        HERE / "runtime_check.py",
        HERE / "fingerprint_check.py",
        DOCUMENTED_EXAMPLES,
    ]
    for fixture in fixtures:
        source = fixture.read_text()
        for forbidden in ("# type: ignore", "cast(", "# pyright:"):
            if forbidden in source:
                raise AssertionError(f"{fixture.name} contains forbidden typing escape {forbidden}")
    for fixture in (HERE / "positive.py", HERE / "runtime_check.py", DOCUMENTED_EXAMPLES):
        source = fixture.read_text()
        for forbidden in ("QueryV2Authority", "declared-schema.json", ".read_bytes()"):
            if forbidden in source:
                raise AssertionError(
                    f"{fixture.name} bypasses generated embedded authority with {forbidden}"
                )

    shutil.rmtree(STAGE, ignore_errors=True)
    STAGE.mkdir(parents=True)
    for fixture in (
        "positive.py",
        "negative.py",
        "runtime_check.py",
        "fingerprint_check.py",
        "authority_rejection_check.py",
        "pyrightconfig.json",
    ):
        shutil.copy2(HERE / fixture, STAGE / fixture)
    shutil.copy2(DOCUMENTED_EXAMPLES, STAGE / "documented_examples.py")

    command(["maturin", "develop"], cwd=CORE)

    command(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "--package",
            "type-bridge-schema-codegen",
            "--example",
            "emit_python_acceptance",
            "--",
            str(HERE / "schema.yaml"),
            str(STAGE / "generated_v2"),
        ]
    )

    variant_source = (
        (HERE / "schema.yaml")
        .read_text()
        .replace(
            "member: { card: { min: 0, max: 2 }, doc: membership player }",
            "member: { card: { min: 0, max: 3 }, doc: membership player }",
        )
    )
    if variant_source == (HERE / "schema.yaml").read_text():
        raise AssertionError("fingerprint variant did not modify the playing fact")
    variant_schema = STAGE / "schema-variant.yaml"
    variant_schema.write_text(variant_source)
    command(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "--package",
            "type-bridge-schema-codegen",
            "--example",
            "emit_python_acceptance",
            "--",
            str(variant_schema),
            str(STAGE / "generated_variant"),
        ]
    )
    command([sys.executable, str(STAGE / "fingerprint_check.py")])
    command([sys.executable, str(STAGE / "authority_rejection_check.py")])

    positive = pyright(STAGE / "positive.py", expected_exit=0)
    if positive["summary"]["errorCount"] != 0:
        raise AssertionError(f"positive Pyright fixture failed: {positive}")

    documented = pyright(STAGE / "documented_examples.py", expected_exit=0)
    if documented["summary"]["errorCount"] != 0:
        raise AssertionError(f"documented Pyright fixture failed: {documented}")

    negative_path = STAGE / "negative.py"
    negative = pyright(negative_path, expected_exit=1)
    check_negative(negative, negative_path)

    command([sys.executable, str(STAGE / "runtime_check.py")])
    print("schema-codegen Python acceptance passed")


if __name__ == "__main__":
    main()
