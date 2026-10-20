"""The private stage recorder exists only in ``perf-stages`` builds."""

from __future__ import annotations

import type_bridge_core


def test_a_default_binding_build_has_no_stage_recorder() -> None:
    # The development extension is a default build, like every shipped wheel.
    assert not hasattr(type_bridge_core, "_perf_stages")
