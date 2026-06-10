"""Strict-mode guard for the cross-language parity gate.

The parity tests shell out to the Node binding and ``pytest.skip(...)`` when the
Node toolchain or its build artifacts are missing. That is the right behavior for
local development, but in CI it would turn the cross-language parity gate into a
silent no-op: a real semantic divergence between the Python and Node sides would
never be reached because the test skipped for want of a build. When
``TYPE_BRIDGE_PARITY_STRICT=1`` (set only by the CI parity job, which builds the
Node prerequisites first), a skip whose reason means "the Node binding was not
built" is promoted to a failure so the gate cannot pass by skipping.
"""

from __future__ import annotations

import os

import pytest

# Skip reasons emitted by the parity suite that mean "Node tooling or its build
# artifacts are missing" (grep-confirmed across cross_language.py,
# descriptor_snapshots.py, test_generator_cross_language.py, and
# test_relates_only_define_parity.py). Matched as substrings so the parameterized
# "(<path>)" suffixes some of them carry still match.
_BUILD_MISSING_MARKERS = (
    "node executable is not installed",
    "npm executable is not installed",
    "native node module not built",
    "compiled Node package not built",
    "compiled generator not built",
    "typed parity reader not built",
    "node is required for the Node descriptor snapshot",
)


def _strict_mode() -> bool:
    return os.environ.get("TYPE_BRIDGE_PARITY_STRICT") == "1"


def _skip_reason(report: pytest.TestReport) -> str:
    # A skip's longrepr is a (fspath, lineno, "Skipped: <reason>") tuple.
    longrepr = report.longrepr
    if isinstance(longrepr, tuple) and len(longrepr) == 3:
        return str(longrepr[2])
    return str(longrepr)


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call: pytest.CallInfo):
    report = yield
    if _strict_mode() and report.skipped:
        reason = _skip_reason(report)
        if any(marker in reason for marker in _BUILD_MISSING_MARKERS):
            report.outcome = "failed"
            report.longrepr = (
                "TYPE_BRIDGE_PARITY_STRICT=1: the Node binding must be built before the "
                "parity gate runs, but this test skipped because it was not. Treating the "
                "skip as a failure so the cross-language gate cannot pass silently. "
                f"Original skip reason: {reason}"
            )
    return report
