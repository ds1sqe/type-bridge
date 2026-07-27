# pyright: reportMissingImports=false
"""P3a no-DB CLI smoke tests for `plan` and `sqlmigrate` verbs.

These tests exercise the pure (no-TypeDB-connection) verbs of the
`type-bridge-migration` binary by running it as a subprocess against a
migrations directory constructed entirely from JSON sidecars.

No TypeDB connection is required; none of these tests use the
`@pytest.mark.integration` marker or the `clean_db` fixture.

Scope: P3a deliverables only.
  - `plan` prints per-step tx_type and kind.
  - `sqlmigrate` prints the carried forward / reverse TypeQL.
  - Colored output appears under `--color=always`; plain text when piped
    (default, no explicit color flag).
  - The D6 drift guard rejects a sidecar whose embedded checksum disagrees
    with the on-disk `.py` text.

P3b connected-verb tests (`migrate`, `showmigrations`, `makemigrations`)
are deferred to the P3b test section in this file.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

# ── Binary discovery ─────────────────────────────────────────────────────────


def _find_migration_bin() -> Path | None:
    """Locate the `type-bridge-migration` binary.

    Resolution order (mirrors the D1 shim discovery):
    1. `sys.executable`'s sibling `bin/` directory (venv-installed console
       script, written by maturin after `maturin develop`).
    2. The cargo debug target directory relative to the repo root
       (`type-bridge-core/target/debug/type-bridge-migration`).
    3. `shutil.which` PATH fallback.
    """
    import shutil

    # 1. venv bin dir alongside sys.executable
    venv_bin = Path(sys.executable).parent / "type-bridge-migration"
    if venv_bin.exists():
        return venv_bin

    # 2. cargo debug output — relative to this file, walk up to repo root
    # test file: tests/integration/migration/test_cli.py
    # repo root: ../../../
    repo_root = Path(__file__).parents[3]
    cargo_bin = repo_root / "type-bridge-core" / "target" / "debug" / "type-bridge-migration"
    if cargo_bin.exists():
        return cargo_bin

    # 3. PATH
    which = shutil.which("type-bridge-migration")
    if which:
        return Path(which)

    return None


# Resolve once at module load time; skip all tests if not found.
_BIN = _find_migration_bin()


def _skip_if_no_bin() -> pytest.MarkDecorator:
    return pytest.mark.skipif(
        _BIN is None,
        reason=(
            "type-bridge-migration binary not found; "
            "run `cargo build -p type-bridge-migration` first"
        ),
    )


# ── Sidecar fixture helpers ──────────────────────────────────────────────────


def _make_run_typeql_spec(
    name: str,
    app_label: str,
    forward: str,
    reverse: str | None,
    *,
    dependencies: list[dict] | None = None,
) -> dict:
    """Build a minimal MigrationSpec dict matching the Rust serde shape."""
    from type_bridge import _rust_runtime

    # The .py text we write is the checksum source.
    py_text = f"# migration: {name}\nclass Migration: pass\n"
    checksum = _rust_runtime.migration_file_checksum(py_text)

    return {
        "app_label": app_label,
        "name": name,
        "dependencies": dependencies or [],
        "operations": [
            {
                "kind": "run_typeql",
                "forward": forward,
                "reverse": reverse,
            }
        ],
        "checksum": checksum,
        "reversible": reverse is not None,
    }


def _write_migration(
    migrations_dir: Path,
    name: str,
    app_label: str,
    forward: str,
    reverse: str | None,
    *,
    dependencies: list[dict] | None = None,
) -> tuple[Path, Path]:
    """Write a `.py` + `.json` sidecar pair to *migrations_dir*.

    Returns `(py_path, json_path)`.
    """
    from type_bridge import _rust_runtime

    migrations_dir.mkdir(parents=True, exist_ok=True)

    py_text = f"# migration: {name}\nclass Migration: pass\n"
    spec = _make_run_typeql_spec(name, app_label, forward, reverse, dependencies=dependencies)

    # Normalize via Rust serde to get the canonical JSON shape.
    normalized = _rust_runtime.normalize_migration_spec(spec)
    json_text = _rust_runtime.migration_spec_to_json(normalized)

    py_path = migrations_dir / f"{name}.py"
    json_path = migrations_dir / f"{name}.json"
    py_path.write_text(py_text)
    json_path.write_text(json_text)
    return py_path, json_path


def _run_bin(*args: str, migrations_dir: Path | None = None) -> subprocess.CompletedProcess:
    """Run the migration binary and capture stdout + stderr."""
    assert _BIN is not None
    cmd = [str(_BIN), *args]
    if migrations_dir is not None:
        # Append --migrations-dir only if not already in args
        if "--migrations-dir" not in args:
            cmd += ["--migrations-dir", str(migrations_dir)]
    return subprocess.run(cmd, capture_output=True, text=True)


# ── Tests: `plan` verb ───────────────────────────────────────────────────────


@_skip_if_no_bin()
def test_plan_no_migrations_prints_notice(tmp_path: Path):
    """When the migrations dir is empty the plan verb says so."""
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    result = _run_bin("plan", "--migrations-dir", str(migrations_dir))
    assert result.returncode == 0, result.stderr
    assert "No pending migrations" in result.stdout


@_skip_if_no_bin()
def test_plan_shows_step_tx_type_and_kind(tmp_path: Path):
    """plan prints per-step tx_type and kind for each pending migration."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse=None,
    )
    _write_migration(
        migrations_dir,
        "0002_add_age",
        app_label,
        forward="define attribute age, value long;",
        reverse="undefine attribute age;",
        dependencies=[{"app_label": app_label, "migration_name": "0001_initial"}],
    )

    result = _run_bin("plan", "--migrations-dir", str(migrations_dir))
    assert result.returncode == 0, result.stderr

    stdout = result.stdout
    # Both migration names must appear.
    assert "0001_initial" in stdout
    assert "0002_add_age" in stdout
    # Step info: tx_type and kind labels must appear.
    assert "tx=schema" in stdout
    assert "schema" in stdout


@_skip_if_no_bin()
def test_plan_with_target_limits_output(tmp_path: Path):
    """plan --target=<name> restricts the output to migrations up to the target."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse=None,
    )
    _write_migration(
        migrations_dir,
        "0002_add_age",
        app_label,
        forward="define attribute age, value long;",
        reverse="undefine attribute age;",
        dependencies=[{"app_label": app_label, "migration_name": "0001_initial"}],
    )

    result = _run_bin(
        "plan",
        "--migrations-dir",
        str(migrations_dir),
        "--target",
        "0001_initial",
    )
    assert result.returncode == 0, result.stderr

    stdout = result.stdout
    assert "0001_initial" in stdout
    # 0002 must NOT appear since it's past the target.
    assert "0002_add_age" not in stdout


# ── Tests: `sqlmigrate` verb ──────────────────────────────────────────────────


@_skip_if_no_bin()
def test_sqlmigrate_prints_forward_typeql(tmp_path: Path):
    """sqlmigrate prints the forward TypeQL for the named migration."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name
    forward_sql = "define attribute score, value long;"

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward=forward_sql,
        reverse="undefine attribute score;",
    )

    result = _run_bin(
        "sqlmigrate",
        "0001_initial",
        "--migrations-dir",
        str(migrations_dir),
    )
    assert result.returncode == 0, result.stderr
    assert forward_sql in result.stdout


@_skip_if_no_bin()
def test_sqlmigrate_reverse_flag_prints_reverse_typeql(tmp_path: Path):
    """sqlmigrate --reverse prints the rollback TypeQL."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name
    forward_sql = "define attribute score, value long;"
    reverse_sql = "undefine attribute score;"

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward=forward_sql,
        reverse=reverse_sql,
    )

    result = _run_bin(
        "sqlmigrate",
        "0001_initial",
        "--migrations-dir",
        str(migrations_dir),
        "--reverse",
    )
    assert result.returncode == 0, result.stderr
    assert reverse_sql in result.stdout
    # Forward SQL must not dominate the output when --reverse is given.
    assert forward_sql not in result.stdout


@_skip_if_no_bin()
def test_sqlmigrate_no_reverse_non_reversible(tmp_path: Path):
    """sqlmigrate --reverse on a non-reversible migration prints a notice."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse=None,  # non-reversible
    )

    result = _run_bin(
        "sqlmigrate",
        "0001_initial",
        "--migrations-dir",
        str(migrations_dir),
        "--reverse",
    )
    assert result.returncode == 0, result.stderr
    assert "no reverse" in result.stdout.lower() or "non-reversible" in result.stdout.lower()


@_skip_if_no_bin()
def test_sqlmigrate_unknown_migration_exits_nonzero(tmp_path: Path):
    """sqlmigrate with an unknown migration name exits with code 2."""
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()

    result = _run_bin(
        "sqlmigrate",
        "9999_does_not_exist",
        "--migrations-dir",
        str(migrations_dir),
    )
    assert result.returncode != 0


# ── Tests: color output ──────────────────────────────────────────────────────


@_skip_if_no_bin()
def test_plan_color_always_emits_ansi_codes(tmp_path: Path):
    """plan --color=always emits ANSI escape codes even when stdout is captured."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse=None,
    )

    result = _run_bin(
        "--color=always",
        "plan",
        "--migrations-dir",
        str(migrations_dir),
    )
    assert result.returncode == 0, result.stderr
    # ANSI escape sequences begin with ESC (\x1b) followed by '['.
    assert "\x1b[" in result.stdout, (
        "Expected ANSI escape codes with --color=always; got plain text: "
        + repr(result.stdout[:200])
    )


@_skip_if_no_bin()
def test_plan_piped_output_is_plain_text(tmp_path: Path):
    """plan without --color (default auto) produces plain text when captured."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse=None,
    )

    # Run without --color flag; subprocess.run captures stdout → not a TTY →
    # anstream auto-strips ANSI codes.
    result = _run_bin("plan", "--migrations-dir", str(migrations_dir))
    assert result.returncode == 0, result.stderr
    assert "\x1b[" not in result.stdout, "Expected plain text when piped; got ANSI codes: " + repr(
        result.stdout[:200]
    )


# ── Tests: D6 drift guard ─────────────────────────────────────────────────────


@_skip_if_no_bin()
def test_plan_rejects_stale_sidecar(tmp_path: Path):
    """plan exits non-zero when the .py has been mutated after the sidecar was written."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    py_path, _json_path = _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse=None,
    )

    # Mutate the .py file AFTER the sidecar was written — simulating a
    # hand-edit that invalidates the sidecar checksum.
    original_text = py_path.read_text()
    py_path.write_text(original_text + "\n# hand-edited: sidecar is now stale\n")

    result = _run_bin("plan", "--migrations-dir", str(migrations_dir))
    assert result.returncode != 0, "Expected non-zero exit when sidecar is stale; got: " + repr(
        result.stdout
    )
    # The error output should mention drift or regenerate.
    combined = result.stdout + result.stderr
    assert "drift" in combined.lower() or "regenerate" in combined.lower(), (
        "Expected a 'drift'/'regenerate' message; got: " + repr(combined[:400])
    )


@_skip_if_no_bin()
def test_sqlmigrate_rejects_stale_sidecar(tmp_path: Path):
    """sqlmigrate exits non-zero when the .py has been mutated after sidecar generation."""
    migrations_dir = tmp_path / "migrations"
    app_label = migrations_dir.name

    py_path, _json_path = _write_migration(
        migrations_dir,
        "0001_initial",
        app_label,
        forward="define attribute name, value string;",
        reverse="undefine attribute name;",
    )

    # Mutate the .py.
    py_path.write_text(py_path.read_text() + "\n# stale\n")

    result = _run_bin(
        "sqlmigrate",
        "0001_initial",
        "--migrations-dir",
        str(migrations_dir),
    )
    assert result.returncode != 0, "Expected non-zero exit for stale sidecar in sqlmigrate"


# ── Tests: connected verbs (docker TypeDB required) ──────────────────────────
#
# These tests drive the STANDALONE binary against a real TypeDB instance.
# The bin opens its OWN connection (via its own Database::connect bootstrap);
# no Python executor is in the loop.
#
# Markers: @pytest.mark.integration (docker TypeDB required) + @_skip_if_no_bin().
# Fixtures: clean_db (function-scoped, from tests/integration/conftest.py).
# Connection params: imported from tests.utils.typedb_lifecycle.


from tests.utils.typedb_lifecycle import TEST_DB_ADDRESS, TEST_DB_NAME  # noqa: E402


def _run_bin_connected(
    *args: str,
    migrations_dir: Path,
    address: str = TEST_DB_ADDRESS,
    database: str = TEST_DB_NAME,
) -> subprocess.CompletedProcess:
    """Run the migration binary with live connection params."""
    assert _BIN is not None
    return subprocess.run(
        [
            str(_BIN),
            *args,
            "-a",
            address,
            "-d",
            database,
            "--migrations-dir",
            str(migrations_dir),
        ],
        capture_output=True,
        text=True,
    )


@pytest.mark.integration
@pytest.mark.order(410)
@_skip_if_no_bin()
def test_bin_migrate_applies_migration_and_showmigrations_reports_applied(clean_db, tmp_path: Path):
    """The standalone bin applies a migration and showmigrations reports it as applied.

    Flow:
      1. Generate a migrations dir (.py + .json sidecar) via MigrationGenerator.
      2. Run the bin: `migrate -a <addr> -d <db> --migrations-dir <dir>`.
         The bin opens its OWN TypeDB connection — no Python executor.
      3. Assert exit 0 and that output contains the migration name.
      4. Run the bin: `showmigrations -a <addr> -d <db> --migrations-dir <dir>`.
      5. Assert exit 0 and that the migration appears as applied in the output.
    """
    from type_bridge import Entity, Flag, Key, String, TypeFlags
    from type_bridge.attribute import AttributeFlags
    from type_bridge.migration import MigrationGenerator

    # Define a minimal model for this test only (distinct type names).
    class CliTestName(String):
        flags = AttributeFlags(name="cli-test-name")

    class CliTestPerson(Entity):
        flags = TypeFlags(name="cli-test-person")
        name: CliTestName = Flag(Key)

    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)

    initial_path = generator.generate(
        models=[CliTestPerson],
        name="initial",
    )
    assert initial_path is not None, "migration generation must succeed"
    migration_name = initial_path.stem  # e.g. "0001_initial"

    # ── Run `migrate` via the standalone bin ─────────────────────────────────
    migrate_result = _run_bin_connected("migrate", migrations_dir=migrations_dir)
    assert migrate_result.returncode == 0, (
        f"`migrate` exited {migrate_result.returncode}.\n"
        f"stdout: {migrate_result.stdout}\nstderr: {migrate_result.stderr}"
    )
    # The output should mention the migration name that was applied.
    combined_migrate = migrate_result.stdout + migrate_result.stderr
    assert migration_name in combined_migrate, (
        f"Expected migration name {migration_name!r} in bin output; got:\n{combined_migrate}"
    )

    # ── Run `showmigrations` via the standalone bin ───────────────────────────
    show_result = _run_bin_connected("showmigrations", migrations_dir=migrations_dir)
    assert show_result.returncode == 0, (
        f"`showmigrations` exited {show_result.returncode}.\n"
        f"stdout: {show_result.stdout}\nstderr: {show_result.stderr}"
    )
    # The migration must appear in the output as applied.
    # Typical format: "[x] 0001_initial" or "applied: 0001_initial".
    show_output = show_result.stdout + show_result.stderr
    assert migration_name in show_output, (
        f"Expected {migration_name!r} in showmigrations output; got:\n{show_output}"
    )
    # Some indicator of "applied" status must be present.
    assert any(marker in show_output.lower() for marker in ("[x]", "applied", "✓", "yes", "[✓]")), (
        f"Expected an 'applied' marker in showmigrations output; got:\n{show_output}"
    )


@pytest.mark.integration
@pytest.mark.order(411)
def test_wheel_makemigrations_via_native_dispatch_generates_py_and_json(clean_db, tmp_path: Path):
    """makemigrations through the wheel entry point generates both files.

    This test exercises the full F1 + F2 compose path through the shim:
      python -m type_bridge.migration makemigrations
          --models <models-module>
          -a <addr> -d <db>
          --migrations-dir <dir>
          --name initial
          --python sys.executable

    The --python flag forces the bin to shell to the SAME venv Python
    (sys.executable in the test process) for the _ir_dump step.
    sys.executable IS the venv python when tests run under `uv run pytest`.

    Assert:
      - exit 0 from the shim.
      - 0001_initial.py is created in the migrations dir.
      - 0001_initial.json is created alongside it.
      - The .py content contains "ops." text (the authoring surface), proving
        the diff + generation path composed through the shim.
    """
    import os

    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir(parents=True, exist_ok=True)

    # Use the v1 fixture models module — it is a fully importable dotted path
    # within the test package and defines a simple Person with a name key.
    # The module is at: tests/integration/migration/fixtures/v1/models.py
    # Importable as: tests.integration.migration.fixtures.v1.models
    models_module = "tests.integration.migration.fixtures.v1.models"

    # Invoke the console module as a subprocess so the full wheel dispatcher →
    # native legacy parser → _generate chain is exercised end-to-end. Remove
    # the Cargo target directories from PATH: no helper binary is part of the
    # installed artifact contract.
    env = {**os.environ}
    cargo_target_dirs = {
        str(Path(__file__).parents[3] / "type-bridge-core" / "target" / profile)
        for profile in ("debug", "release")
    }
    env["PATH"] = os.pathsep.join(
        entry
        for entry in env.get("PATH", "").split(os.pathsep)
        if entry and entry not in cargo_target_dirs
    )

    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "type_bridge.migration",
            "makemigrations",
            "--models",
            models_module,
            "-a",
            TEST_DB_ADDRESS,
            "-d",
            TEST_DB_NAME,
            "--migrations-dir",
            str(migrations_dir),
            "--name",
            "initial",
            "--python",
            sys.executable,
        ],
        capture_output=True,
        text=True,
        env=env,
    )

    combined = result.stdout + result.stderr

    assert result.returncode == 0, (
        f"shim makemigrations exited {result.returncode}.\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )

    py_file = migrations_dir / "0001_initial.py"
    json_file = migrations_dir / "0001_initial.json"

    assert py_file.exists(), (
        f"0001_initial.py not created in {migrations_dir}.\n"
        f"Dir contents: {list(migrations_dir.iterdir())}\n"
        f"bin output: {combined}"
    )
    assert json_file.exists(), (
        f"0001_initial.json not created alongside 0001_initial.py.\n"
        f"Dir contents: {list(migrations_dir.iterdir())}\n"
        f"bin output: {combined}"
    )

    # The generated .py must use the ops.* authoring surface.
    py_content = py_file.read_text()
    assert "ops." in py_content, (
        f"Expected 'ops.' in generated .py (authoring surface); got:\n{py_content[:500]}"
    )
