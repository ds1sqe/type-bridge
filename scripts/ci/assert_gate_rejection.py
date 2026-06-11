#!/usr/bin/env python3
"""Assert that the TypeBridge version gate rejects a connect with the expected error class.

CI's rejection cells call this against a live TypeDB service container. A cell
is green when ``Database.connect()`` raises at connect time (before any driver
construction or transaction) with the expected error class, and the message
respects the human-version contract (no protocol band numbers, no ``0.0.0``).

Usage:
    assert_gate_rejection.py --address localhost:1729 --expect-class embedded
    assert_gate_rejection.py --address localhost:1729 --expect-class installed
    assert_gate_rejection.py --address localhost:1729 --expect-class window
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
        "--expect-class",
        required=True,
        choices=sorted(CLASS_MARKERS),
        help="Which gate check must have produced the rejection",
    )
    args = parser.parse_args()

    import type_bridge_core

    from type_bridge.session import Database

    db = Database(address=args.address, database="ci_gate_rejection_probe")
    try:
        db.connect()
    except type_bridge_core.VersionError as exc:
        msg = str(exc)
        print(f"gate fired at connect: {msg}")
        marker = CLASS_MARKERS[args.expect_class]
        if marker not in msg:
            print(f"FAIL: expected the {args.expect_class!r} class marker {marker!r}")
            return 1
        if args.expect_class == "installed" and CLASS_MARKERS["embedded"] in msg:
            print("FAIL: installed-class rejection carries the embedded framing")
            return 1
        for forbidden in FORBIDDEN:
            if forbidden in msg:
                print(f"FAIL: message exposes forbidden token {forbidden!r}")
                return 1
        print(f"OK: connect-time rejection with the {args.expect_class!r} error class")
        return 0

    print("FAIL: connect() succeeded — the gate did not fire")
    return 1


if __name__ == "__main__":
    sys.exit(main())
