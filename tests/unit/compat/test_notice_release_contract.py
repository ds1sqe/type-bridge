"""Keep the expanded notice exact and the maintenance release non-removing."""

import hashlib
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
NOTICE = ROOT / "docs/guide/v2.0.2-notice.md"
# Approved #189 inventory, including the explicit mixed-purpose retention rules.
APPROVED_INVENTORY_SHA256 = "f21ef7ef8c063cd079253106179729adbe63ee8df265ade7fdc7051aecc1fef2"


def test_notice_reproduces_the_entire_approved_inventory() -> None:
    text = NOTICE.read_text()
    inventory = "## Exact Public Inventory\n" + text.split("## Exact Public Inventory\n", 1)[1]
    assert hashlib.sha256(inventory.rstrip().encode()).hexdigest() == APPROVED_INVENTORY_SHA256


def test_notice_is_nonretroactive_and_does_not_add_warning_behavior() -> None:
    text = NOTICE.read_text()
    for marker in (
        "Neither 2.0.0 nor 2.0.1 provided this",
        "No API is removed in 2.0.2",
        "no new handwritten-authoring warning class",
        "type-bridge>=2,<2.1",
        '"@type-bridge/node": ">=2 <2.1"',
        'type-bridge = "=2.0.2"',
        "does not backport that implementation",
    ):
        assert marker in text
    assert "guide/v2.0.2-notice.md" in (ROOT / "mkdocs.yml").read_text()


def test_notice_release_has_one_tag_and_matching_signing_identity() -> None:
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    parsed = yaml.load(workflow, Loader=yaml.BaseLoader)
    assert parsed["on"]["push"]["tags"] == ["v2.0.2"]
    assert "release.yml@refs/tags/v2[.]0[.]2$" in workflow
    assert "release.yml@refs/tags/v2[.]0[.]0$" not in workflow
    assert "'docs/guide/v2.0.2-notice.md' || 'dist/server-oci-release.md'" in workflow
    # Frozen old recovery remains separately selected; it cannot become 2.0.2.
    assert "inputs.release_channel == 'recovery' && 'v2.0.0' || 'v2.0.2'" in workflow
