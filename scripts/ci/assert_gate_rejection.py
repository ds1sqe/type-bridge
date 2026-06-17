#!/usr/bin/env python3
"""Assert a TypeBridge version-gate CI matrix cell.

CI calls this against a live TypeDB service container. Rejection cells are green
when the requested probe path raises with the expected error class, and the
message respects the human-version contract (no protocol band numbers, no
``0.0.0``). Positive cells are green when the requested probe path succeeds.

Usage:
    assert_gate_rejection.py --address localhost:1729 --probe connect --expect window
    assert_gate_rejection.py --address localhost:1729 --probe driver --expect installed
    assert_gate_rejection.py --address localhost:1729 --probe connect --http-port 1 --expect ok
"""

from __future__ import annotations

import argparse
import sys

# Substrings that identify which gate check produced the rejection.
CLASS_MARKERS = {
    # The embedded Rust runtime driver check — wheel-appropriate remediation.
    "embedded": "TypeBridge's embedded runtime",
    # The installed Python driver band check — pip-level remediation.
    "installed": "is not protocol-compatible with",
    # The support-window check (either component below floor / above ceiling).
    "window": "outside the supported window",
}

# These must never appear in any user-facing gate message.
FORBIDDEN = ["band 7", "band 8", "0.0.0"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True, help="TypeDB gRPC address, e.g. localhost:1729")
    parser.add_argument(
        "--probe",
        required=True,
        choices=["connect", "driver"],
        help="Connection path to exercise: Database.connect() or Database.driver",
    )
    parser.add_argument(
        "--expect-class",
        required=False,
        choices=sorted(CLASS_MARKERS),
        help="Deprecated alias for --expect when expecting a rejection class",
    )
    parser.add_argument(
        "--expect",
        required=False,
        choices=["ok", *sorted(CLASS_MARKERS)],
        help="Expected outcome: ok for success, otherwise the rejection class",
    )
    parser.add_argument(
        "--http-port",
        type=int,
        default=8000,
        help="HTTP version-probe port to pass into Database",
    )
    args = parser.parse_args()
    expect = args.expect or args.expect_class
    if expect is None:
        parser.error("--expect is required")

    import type_bridge_core

    from type_bridge.session import Database

    db = Database(
        address=args.address,
        database="ci_gate_rejection_probe",
        http_port=args.http_port,
    )
    try:
        if args.probe == "driver":
            _ = db.driver
        else:
            db.connect()
    except type_bridge_core.VersionError as exc:
        if expect == "ok":
            print(f"FAIL: {args.probe} rejected unexpectedly: {exc}")
            return 1
        msg = str(exc)
        print(f"gate fired at {args.probe}: {msg}")
        marker = CLASS_MARKERS[expect]
        if marker not in msg:
            print(f"FAIL: expected the {expect!r} class marker {marker!r}")
            return 1
        if expect == "installed" and CLASS_MARKERS["embedded"] in msg:
            print("FAIL: installed-class rejection carries the embedded framing")
            return 1
        for forbidden in FORBIDDEN:
            if forbidden in msg:
                print(f"FAIL: message exposes forbidden token {forbidden!r}")
                return 1
        print(f"OK: version-gate rejection with the {expect!r} error class")
        return 0

    db.close()
    if expect == "ok":
        print(f"OK: {args.probe} succeeded")
        return 0

    print(f"FAIL: {args.probe} succeeded - the gate did not fire")
    return 1


if __name__ == "__main__":
    sys.exit(main())
