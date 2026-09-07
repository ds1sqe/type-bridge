"""Keep the expanded notice exact and the maintenance release non-removing."""

import hashlib
import json
import os
import shlex
import subprocess
from pathlib import Path

import pytest
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


@pytest.mark.parametrize(
    ("channel", "tag_object", "ref_type", "target_type", "target_revision", "accepted"),
    [
        ("stable", "b" * 40, "tag", "commit", "c" * 40, True),
        ("stable", "b" * 40, "tag", "commit", "d" * 40, False),
        ("stable", "b" * 40, "commit", "commit", "c" * 40, False),
        ("stable", "b" * 40, "tag", "tag", "c" * 40, False),
        ("recovery", "a4cec6478ad4e764f039e51eabcbb68d45efd45a", "tag", "commit", "c" * 40, True),
        ("recovery", "b" * 40, "tag", "commit", "c" * 40, False),
        ("recovery", "a4cec6478ad4e764f039e51eabcbb68d45efd45a", "tag", "commit", "d" * 40, False),
    ],
)
def test_oci_publisher_binds_current_tag_and_preserves_frozen_recovery(
    channel: str,
    tag_object: str,
    ref_type: str,
    target_type: str,
    target_revision: str,
    accepted: bool,
) -> None:
    workflow = yaml.load(
        (ROOT / ".github/workflows/release.yml").read_text(), Loader=yaml.BaseLoader
    )
    steps = workflow["jobs"]["publish-server-oci"]["steps"]
    guards = [step["run"] for step in steps if step["name"] == "Revalidate immutable release tag"]
    assert len(guards) == 1
    release_tag = "v2.0.0" if channel == "recovery" else "v2.0.2"
    ref_payload = json.dumps({"object": {"type": ref_type, "sha": tag_object}})
    tag_payload = json.dumps({"object": {"type": target_type, "sha": target_revision}})
    # Execute the actual publisher guard with read-only GitHub responses mocked.
    mock_gh = f"""
gh() {{
    test "$1" = api || return 2
    case "$2" in
        repos/ds1sqe/type-bridge/git/ref/tags/{release_tag})
            printf '%s\\n' {shlex.quote(ref_payload)} ;;
        repos/ds1sqe/type-bridge/git/tags/{tag_object})
            printf '%s\\n' {shlex.quote(tag_payload)} ;;
        *) return 2 ;;
    esac
}}
"""
    result = subprocess.run(
        ["bash", "-c", mock_gh + guards[0]],
        env={
            **os.environ,
            "GITHUB_REPOSITORY": "ds1sqe/type-bridge",
            "RELEASE_TAG": release_tag,
            "RELEASE_CHANNEL": channel,
            "RELEASE_REVISION": "c" * 40,
        },
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    assert (result.returncode == 0) is accepted, result.stderr
