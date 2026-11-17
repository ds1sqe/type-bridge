"""Protect the current release workflow and immutable tag guard."""

import json
import os
import shlex
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]


def test_current_release_keeps_the_published_notice_separate() -> None:
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    parsed = yaml.load(workflow, Loader=yaml.BaseLoader)
    assert parsed["on"]["push"]["tags"] == ["v3.0.0"]
    assert "release.yml@refs/tags/${{ env.RELEASE_TAG }}" in workflow
    assert "release.yml@refs/tags/v2[.]0[.]0$" not in workflow
    jobs = parsed["jobs"]
    current = jobs["github-release"]
    release = next(step for step in current["steps"] if "softprops/" in step.get("uses", ""))
    assert release["with"]["body_path"] == "dist/server-oci-release.md"
    assert not any("notice" in name or "recovery" in name for name in jobs)


@pytest.mark.parametrize(
    ("channel", "tag_object", "ref_type", "target_type", "target_revision", "accepted"),
    [
        ("stable", "b" * 40, "tag", "commit", "c" * 40, True),
        ("stable", "e" * 40, "tag", "commit", "c" * 40, False),
        ("stable", "b" * 40, "tag", "commit", "d" * 40, False),
        ("stable", "b" * 40, "commit", "commit", "c" * 40, False),
        ("stable", "b" * 40, "tag", "tag", "c" * 40, False),
    ],
)
def test_oci_publisher_binds_current_tag(
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
    release_tag = "v3.0.0"
    expected_tag = "b" * 40
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
            "EXPECTED_RELEASE_TAG_OBJECT": expected_tag,
        },
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    assert (result.returncode == 0) is accepted, result.stderr
