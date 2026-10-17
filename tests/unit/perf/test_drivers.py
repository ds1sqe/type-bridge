"""The SDK pipeline drivers measure the Rust driver's pipelines and schema.

The drivers themselves run against a live TypeDB (see perf.yml's
``pipeline`` job); these checks keep them aligned with the Rust driver and
check the shared schema and seed sizes.
"""

from __future__ import annotations

import re
from pathlib import Path

from perf.drivers import workspace
from perf.drivers.python import pipelines as python_driver

ROOT = Path(__file__).resolve().parents[3]
RUST_DRIVER = ROOT / "type-bridge-core/crates/perf/src/bin/pipelines.rs"
PURCHASE_PLAN = ROOT / "type-bridge-core/crates/rust/tests/support/purchase_plan.rs"
NODE_DRIVER = ROOT / "perf/drivers/node/pipelines.cjs"


def _rust_pipelines() -> dict[str, str]:
    source = RUST_DRIVER.read_text(encoding="utf-8")
    start = source.index("const PIPELINES")
    table = source[start : source.index("];", start)]
    return dict(re.findall(r'\("([a-z0-9_]+)", ([A-Z0-9_]+)\)', table))


def test_the_workspace_schema_is_the_rust_purchase_source() -> None:
    source = PURCHASE_PLAN.read_text(encoding="utf-8")
    rust = re.search(r'pub const SOURCE: &str = "(.*?)";\n', source, re.S)
    assert rust is not None
    shipped = (workspace.SCHEMA / "fragments/purchase.yaml").read_text(encoding="utf-8")
    assert shipped == rust.group(1)
    assert f'pub const SCOPE: &str = "{workspace.SCOPE}";' in source
    assert f'pub const PROFILE: &str = "{workspace.PROFILE}";' in source


def test_the_sdk_drivers_run_the_rust_data_pipelines_in_order() -> None:
    rust = _rust_pipelines()
    # Migration apply is a CLI operation; no other SDK has an entry for it.
    data = [name for name in rust if name != "migration_apply"]
    assert list(python_driver.PIPELINES) == data
    assert python_driver.PIPELINES["batch_given_rows_1k"] == python_driver.BATCH_ROWS
    source = NODE_DRIVER.read_text(encoding="utf-8")
    start = source.index("const PIPELINES = [")
    node = re.findall(r'\["([a-z0-9_]+)", ', source[start : source.index("];", start)])
    assert node == data


def test_the_drivers_keep_the_rust_seed_sizes() -> None:
    source = RUST_DRIVER.read_text(encoding="utf-8")
    node = NODE_DRIVER.read_text(encoding="utf-8")
    for name in ("SEEDED_ITEMS", "FILTER_STOCK", "BATCH_ROWS", "MIXED_BATCH_ROWS", "DOCTORS"):
        found = re.search(rf"const {name}: \w+ = ([0-9_]+);", source)
        assert found is not None, name
        value = int(found.group(1).replace("_", ""))
        assert getattr(python_driver, name) == value, name
        assert f"const {name} = {value};" in node, name
