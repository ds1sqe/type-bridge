from datetime import UTC, date, datetime, timedelta
from decimal import Decimal

from generated_v2 import (
    Container,
    Employment,
    Event,
    EventRef,
    Identifier,
    Membership,
    Person,
    PersonRef,
    RoleToken,
    Score,
    ValBool,
    ValConstrained,
    ValDate,
    ValDatetime,
    ValDatetimeTz,
    ValDecimal,
    ValDouble,
    ValDuration,
)


def person(identifier: Identifier) -> Person:
    return Person(
        identifier=identifier,
        score=Score(3),
        val_bool=ValBool(True),
        val_constrained=ValConstrained(20),
        val_date=ValDate(date(2026, 7, 29)),
        val_datetime=ValDatetime(datetime(2026, 7, 29)),
        val_datetime_tz=ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        val_decimal=ValDecimal(Decimal("3.5")),
        val_double=ValDouble(3.5),
        val_duration=ValDuration(timedelta(seconds=3)),
    )


def accepts_employment_role(role: RoleToken[Employment, Person]) -> None:
    del role


Event()  # E: missing_event_subject:reportCallIssue
Employment()  # E: missing_required:reportCallIssue
Employment(
    employee=PersonRef(  # E: reference_as_complete:reportArgumentType
        "person-iid", identifier=Identifier("person-1")
    )
)
Employment(
    employee=person(Identifier("person-1")),
    member=person(Identifier("person-2")),  # E: specialized_keyword:reportCallIssue
)
accepts_employment_role(Membership.member)  # E: wrong_owner:reportArgumentType
Container(item=EventRef("event-iid"))  # E: scalar_for_sequence:reportArgumentType
Employment(
    employee=[  # E: sequence_for_scalar:reportArgumentType
        person(Identifier("person-1"))
    ]
)
person(7)  # E: wrong_scalar:reportArgumentType
