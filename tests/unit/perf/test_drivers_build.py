"""``perf.drivers.build``: one build of every SDK driver with perf-stages."""

from __future__ import annotations

from pathlib import Path

from perf.drivers import build


def test_every_binding_builds_with_perf_stages(tmp_path: Path) -> None:
    lines = [" ".join(command) for command, _ in build.commands(tmp_path, {})]

    assert any("-p type-bridge-node --features perf-stages" in line for line in lines)
    assert any("-p type-bridge-c --features perf-stages" in line for line in lines)
    assert any("maturin build" in line and "perf-stages" in line for line in lines)
    # A default build of a binding would replace the artifact a driver loads.
    for command, _ in build.commands(tmp_path, {}):
        if {"type-bridge-node", "type-bridge-c", "type-bridge-core"} & set(command):
            assert "perf-stages" in command


def test_the_c_driver_links_the_copied_library(tmp_path: Path) -> None:
    commands = build.commands(tmp_path, {})
    (cc, _) = commands[-1]
    copy = next(i for i, (command, _) in enumerate(commands) if "type-bridge-c" in command)

    assert cc[0] == "cc"
    assert copy < len(commands) - 1
    assert f"-Wl,-rpath,{tmp_path / 'clib'}" in cc
    assert str(build.ROOT / "perf/drivers/c/pipelines.c") in cc


def test_cargo_target_dir_moves_the_release_directory(tmp_path: Path) -> None:
    assert build.target_dir({"CARGO_TARGET_DIR": str(tmp_path)}) == tmp_path / "release"
    assert build.target_dir({}) == build.CORE / "target/release"


def test_artifacts_are_copied_out_after_their_build(tmp_path: Path) -> None:
    release = tmp_path / "target/release"
    release.mkdir(parents=True)
    (release / "libtype_bridge_node.so").write_bytes(b"node")
    (release / "libtype_bridge_c.so").write_bytes(b"c")
    env = {"CARGO_TARGET_DIR": str(tmp_path / "target")}
    out = tmp_path / "out"
    out.mkdir()

    build.copy_artifact(["cargo", "build", "-p", "type-bridge-node"], out, env)
    build.copy_artifact(["cargo", "build", "-p", "type-bridge-c"], out, env)

    assert (out / "type_bridge_node.node").read_bytes() == b"node"
    assert (out / "clib/libtype_bridge_c.so").read_bytes() == b"c"
