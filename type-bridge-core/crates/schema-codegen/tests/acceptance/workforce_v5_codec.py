#!/usr/bin/env python3
"""Publish Python's generated provider-free Workforce V5 canonical corpus."""

from __future__ import annotations

import base64
import json
import os
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path

import generated_ordered as generated

FORMAT = "typebridge.workforce-v5-provider-free-corpus/v1"


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


def main() -> None:
    output_value = os.environ.get("TYPE_BRIDGE_WORKFORCE_V5_CORPUS")
    if output_value is None:
        raise RuntimeError("TYPE_BRIDGE_WORKFORCE_V5_CORPUS must be configured")
    output = Path(output_value)
    if not output.is_absolute():
        raise RuntimeError("Workforce V5 corpus path must be absolute")

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
    with output.open("xb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


if __name__ == "__main__":
    main()
