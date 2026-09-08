#!/usr/bin/env python3
"""Publish Python's generated provider-free Sdk V5 canonical corpus."""

from __future__ import annotations

import base64
import json
import os
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path

import generated_ordered as generated
import generated_ordered_foreign as foreign

FORMAT = "typebridge.sdk-v5-provider-free-corpus/v1"
OPERATIONAL_FORMAT = "typebridge.sdk-v5-operational-evidence/v1"


def person(*, aliases: list[generated.Aliases] | None = None) -> generated.Person:
    values = {} if aliases is None else {"aliases": aliases}
    value = generated.Person(
        **values,
        identifier=generated.Identifier("person-v5"),
        score=generated.Score(42),
        score__gte=generated.ScoreGte(7),
        val_bool=generated.ValBool(True),
        val_constrained=generated.ValConstrained(50),
        val_date=generated.ValDate(date(2026, 8, 22)),
        val_datetime=generated.ValDatetime(datetime(2026, 8, 22, 12, 34, 56)),
        val_datetime_tz=generated.ValDatetimeTz(datetime(2026, 8, 22, 12, 34, 56, tzinfo=UTC)),
        val_decimal=generated.ValDecimal(Decimal("123.45")),
        val_double=generated.ValDouble(-0.0),
        val_duration=generated.ValDuration(timedelta(days=1)),
    )
    if aliases is None:
        value.runtime_values().pop("foo__bar")
        value.runtime_values().pop("nickname")
    return value


def canonical_json(value: object) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode()


def expect_code(operation: object, expected: str) -> Exception:
    try:
        assert callable(operation)
        operation()
    except Exception as error:
        if getattr(error, "code", None) != expected:
            raise AssertionError(f"expected {expected}, saw {error!r}") from error
        return error
    raise AssertionError(f"controlled canonical operation must fail with {expected}")


def publish(path: Path, payload: bytes) -> None:
    if not path.is_absolute():
        raise RuntimeError("Sdk V5 evidence path must be absolute")
    with path.open("xb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


def main() -> None:
    output_value = os.environ.get("TYPE_BRIDGE_SDK_V5_CORPUS")
    if output_value is None:
        raise RuntimeError("TYPE_BRIDGE_SDK_V5_CORPUS must be configured")
    output = Path(output_value)
    operational_value = os.environ.get("TYPE_BRIDGE_SDK_V5_OPERATIONAL_EVIDENCE")
    if operational_value is None:
        raise RuntimeError("TYPE_BRIDGE_SDK_V5_OPERATIONAL_EVIDENCE must be configured")
    operational_output = Path(operational_value)

    integer_key = generated.RobotId(9_007_199_254_740_993).encode_attribute()
    stats = generated.PlayerStats(nickname="stable", wins=3).encode()
    person_create_value = person(aliases=[])
    person_create = person_create_value.encode_create()
    person_snapshot_value = person()
    person_snapshot_value.attach_runtime_iid("0x501")
    person_snapshot = person_snapshot_value.encode_snapshot()

    robot = generated.Robot(
        robot_id=generated.RobotId(9_007_199_254_740_993),
        val_constrained=generated.ValConstrained(50),
    )
    membership = generated.Membership(member=robot).encode_create()
    interaction = generated.Interaction(
        identifier=generated.Identifier("interaction-v5"),
        nickname=generated.Nickname("Ada"),
        actor=robot,
        target=person_create_value,
    ).encode_create()
    event = generated.EventRef("0x700")
    container = generated.Container(item=[event]).encode_create()
    employment_player = person()
    employment_player.attach_runtime_iid("0x501")
    employment_value = generated.Employment(employee=employment_player)
    employment_value.attach_runtime_iid("0x801")
    employment_snapshot = employment_value.encode_snapshot()
    player_reference = event.encode_reference()

    records = [
        integer_key,
        stats,
        person_create,
        person_snapshot,
        membership,
        interaction,
        container,
        employment_snapshot,
        player_reference,
    ]
    for index, record in enumerate(records):
        try:
            generated.encode_archive([record])
        except Exception as error:
            raise AssertionError(f"Python record {index} failed archive admission") from error
    archive = generated.encode_archive(records)
    decoded = generated.decode_archive(archive)
    if decoded != records:
        raise AssertionError("Python archive did not preserve exact record bytes")
    if generated.Person.decode_snapshot(decoded[3]).encode_snapshot() != decoded[3]:
        raise AssertionError("Python person snapshot did not re-encode byte-exactly")
    if generated.Employment.decode_snapshot(decoded[7]).encode_snapshot() != decoded[7]:
        raise AssertionError("Python employment snapshot did not re-encode byte-exactly")

    cancellation = generated.QueryCancellation()
    cancellation.cancel()
    cancellation_error = expect_code(
        lambda: generated.encode_archive_controlled(records, cancellation=cancellation),
        "projected_codec_cancelled",
    )
    input_error = expect_code(
        lambda: generated.decode_archive_controlled(archive, max_input_bytes=len(archive) - 1),
        "projected_codec_input_limit",
    )
    output_error = expect_code(
        lambda: generated.encode_archive_controlled(records, max_output_bytes=len(archive) - 1),
        "projected_codec_output_limit",
    )
    member_error = expect_code(
        lambda: generated.encode_archive_controlled(records, max_records=len(records) - 1),
        "projected_codec_member_limit",
    )
    depth_error = expect_code(
        lambda: generated.decode_archive_controlled(archive, max_depth=1),
        "projected_codec_depth_limit",
    )
    deadline_error = expect_code(
        lambda: generated.decode_archive_controlled(archive, timeout_milliseconds=0),
        "projected_codec_deadline_exceeded",
    )
    foreign_record = foreign.RobotId(7).encode_attribute()
    foreign_error = expect_code(
        lambda: generated.encode_archive([foreign_record]),
        "projected_record_schema_mismatch",
    )
    if getattr(foreign_error, "sdk_category", None) != "invalid_input":
        raise AssertionError("foreign-schema diagnostic category drifted")
    if getattr(foreign_error, "path", None) != [
        {"kind": "contract_field", "value": "declared_schema_identity"}
    ]:
        raise AssertionError("foreign-schema diagnostic path drifted")
    if getattr(foreign_error, "details", None) != {}:
        raise AssertionError("foreign-schema diagnostic exposed payload details")

    sibling_archive = generated.encode_archive(records)
    sibling_records = generated.decode_archive(sibling_archive)
    if sibling_records != records:
        raise AssertionError("independent Python archive was unusable after failures")
    del sibling_records
    del sibling_archive

    operational_payload = canonical_json(
        {
            "binding": "python",
            "cancellation": {"code": cancellation_error.code, "partial_output": False},
            "deadline": {"code": deadline_error.code, "partial_output": False},
            "diagnostic": {
                "category": foreign_error.sdk_category,
                "code": foreign_error.code,
                "path": ["declared_schema_identity"],
                "payload_absent": True,
            },
            "format": OPERATIONAL_FORMAT,
            "lifecycle": {
                "archive_closed": True,
                "builder_closed": True,
                "bytes_closed": True,
                "decoded_closed": True,
                "repeat_close": True,
                "sibling_usable": True,
            },
            "resource_limits": {
                "depth_code": depth_error.code,
                "input_code": input_error.code,
                "member_code": member_error.code,
                "output_code": output_error.code,
                "partial_output": False,
            },
            "test_id": "python.generated_sdk_v5_canonical_codec",
        }
    )

    def encoded(value: bytes) -> str:
        return base64.b64encode(value).decode("ascii")

    payload = canonical_json(
        {
            "archive_b64": encoded(archive),
            "binding": "python",
            "format": FORMAT,
            "record_b64": [encoded(record) for record in records],
        }
    )
    publish(output, payload)
    publish(operational_output, operational_payload)


if __name__ == "__main__":
    main()
