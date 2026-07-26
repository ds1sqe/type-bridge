"""Live acceptance coverage for the immutable typed-query facade.

The fixture intentionally uses Python field names that differ from their
TypeDB attribute labels.  This keeps the live compiler honest: descriptor
metadata, rather than Python-side field keys, must own every emitted label.
"""

from __future__ import annotations

from dataclasses import FrozenInstanceError, dataclass

import pytest
import type_bridge_core

from type_bridge import (
    AttributeFlags,
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)
from type_bridge.typed import QuerySession


class LiveName(String):
    flags = AttributeFlags(name="typed-live-name")


class LiveRank(Integer):
    flags = AttributeFlags(name="typed-live-rank")


class LiveTag(String):
    flags = AttributeFlags(name="typed-live-tag")


class LiveCode(String):
    flags = AttributeFlags(name="typed-live-code")


class LivePerson(Entity):
    flags = TypeFlags(name="typed-live-person")
    display_name: LiveName = Flag(Key)
    ranking: LiveRank | None = None
    labels: list[LiveTag] = Flag(Card(min=0))


class LiveEmployee(LivePerson):
    flags = TypeFlags(name="typed-live-employee")


class LiveCompany(Entity):
    flags = TypeFlags(name="typed-live-company")
    display_name: LiveName = Flag(Key)


class LiveEmployment(Relation):
    flags = TypeFlags(name="typed-live-employment")
    external_code: LiveCode = Flag(Key)
    employee: Role[LivePerson] = Role("employee", LivePerson)
    employer: Role[LiveCompany] = Role("employer", LiveCompany)
    reviewers: Role[LivePerson] = Role(
        "reviewer",
        LivePerson,
        cardinality=Card(min=0),
    )


class LiveComposition(Relation):
    flags = TypeFlags(name="typed-live-composition")
    parent: Role[LivePerson] = Role("src", LivePerson)
    child: Role[LivePerson] = Role("dst", LivePerson)


LIVE_PERSON_NAME = LiveName
LIVE_PERSON_LABELS = LiveTag
LIVE_PERSON_RANK = LiveRank
LIVE_COMPANY_NAME = LiveName
LIVE_EMPLOYMENT_CODE = LiveCode


@dataclass(frozen=True, slots=True)
class LivePersonWorkPage:
    person: LivePerson
    employments: tuple[LiveEmployment, ...]
    companies: tuple[LiveCompany, ...]


@pytest.fixture
def typed_query_graph(clean_db):
    schema = SchemaManager(clean_db)
    schema.register(
        LivePerson,
        LiveEmployee,
        LiveCompany,
        LiveEmployment,
        LiveComposition,
    )
    schema.sync_schema(force=True)
    clean_db.execute_query(
        """
        insert
        $alice isa typed-live-employee,
            has typed-live-name "Alice",
            has typed-live-rank 2,
            has typed-live-tag "blue",
            has typed-live-tag "red";
        $bob isa typed-live-person,
            has typed-live-name "Bob",
            has typed-live-rank 1,
            has typed-live-tag "blue";
        $carol isa typed-live-person,
            has typed-live-name "Carol",
            has typed-live-tag "green";
        $dave isa typed-live-employee,
            has typed-live-name "Dave",
            has typed-live-rank 3,
            has typed-live-tag "red";
        $acme isa typed-live-company, has typed-live-name "Acme";
        $globex isa typed-live-company, has typed-live-name "Globex";
        $e1 isa typed-live-employment,
            links (employee: $alice, employer: $acme, reviewer: $dave),
            has typed-live-code "E-02";
        $e2 isa typed-live-employment,
            links (employee: $alice, employer: $globex,
                reviewer: $bob, reviewer: $dave),
            has typed-live-code "E-01";
        $e3 isa typed-live-employment,
            links (employee: $bob, employer: $acme, reviewer: $alice),
            has typed-live-code "E-03";
        $e4 isa typed-live-employment,
            links (employee: $dave, employer: $acme, reviewer: $alice),
            has typed-live-code "E-04";
        $c1 isa typed-live-composition,
            links (src: $alice, dst: $bob);
        $c2 isa typed-live-composition,
            links (src: $alice, dst: $carol);
        $c3 isa typed-live-composition,
            links (src: $bob, dst: $dave);
        $c4 isa typed-live-composition,
            links (src: $carol, dst: $dave);
        $c5 isa typed-live-composition,
            links (src: $dave, dst: $alice);
        """,
        transaction_type="write",
    )
    return clean_db


def _name(value: LivePerson | LiveCompany) -> str:
    return value.display_name.value


def _people(value: object) -> list[LivePerson]:
    if not isinstance(value, list):
        raise AssertionError("expected a hydrated role-player list")
    people: list[LivePerson] = []
    for item in value:
        if not isinstance(item, LivePerson):
            raise AssertionError("expected a LivePerson role player")
        people.append(item)
    return people


@pytest.mark.integration
def test_typed_rows_execute_exact_and_subtype_targets(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    exact_person = session.var(LivePerson)
    exact_names = [
        _name(person)
        for person in session.query(exact_person).rows(
            limit=10,
            order_by=(exact_person.field(LIVE_PERSON_NAME).asc(),),
        )
    ]
    assert exact_names == ["Bob", "Carol"]

    any_person = session.var(LivePerson, subtypes=True)
    all_people = session.query(any_person).rows(
        limit=10,
        order_by=(any_person.field(LIVE_PERSON_NAME).asc(),),
    )
    assert [_name(person) for person in all_people] == ["Alice", "Bob", "Carol", "Dave"]
    assert [type(person) for person in all_people] == [
        LiveEmployee,
        LivePerson,
        LivePerson,
        LiveEmployee,
    ]
    assert {label.value for label in all_people[0].labels} == {"blue", "red"}
    assert all_people[2].ranking is None


@pytest.mark.integration
def test_typed_one_boolean_predicates_and_cardinality_errors(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    person = session.var(LivePerson, subtypes=True)
    name = person.field(LIVE_PERSON_NAME)

    alice = session.query(person).where(name.eq(LiveName("Alice"))).one()
    assert type(alice) is LiveEmployee
    assert _name(alice) == "Alice"

    boolean_rows = (
        session.query(person)
        .where(
            (name.eq(LiveName("Alice")) | name.eq(LiveName("Bob"))) & ~name.eq(LiveName("Carol"))
        )
        .rows(limit=10, order_by=(name.asc(),))
    )
    assert [_name(value) for value in boolean_rows] == ["Alice", "Bob"]

    with pytest.raises(type_bridge_core.MatchRequestError) as missing:
        session.query(person).where(name.eq(LiveName("Nobody"))).one()
    assert missing.value.code == "no_result"

    with pytest.raises(type_bridge_core.MatchRequestError) as multiple:
        session.query(person).one()
    assert multiple.value.code == "not_unique"


@pytest.mark.integration
def test_typed_string_predicates_use_version_adaptive_parameter_transport(
    typed_query_graph,
) -> None:
    """Band 9 uses one ``given`` row; older bands retain typed inline lowering."""
    session = QuerySession(typed_query_graph)
    person = session.var(LivePerson, subtypes=True)
    name = person.field(LIVE_PERSON_NAME)

    alice = (
        session.query(person)
        .where(
            name.starts_with(LiveName("Al")),
            name.contains(LiveName("LIC")),
            name.ends_with(LiveName("ice")),
            name.regex(LiveName(r"^A[[:alpha:]]+$")),
        )
        .one()
    )

    assert type(alice) is LiveEmployee
    assert _name(alice) == "Alice"


@pytest.mark.integration
def test_typed_repeated_models_field_comparison_and_stable_offset(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    left = session.var(LivePerson, subtypes=True)
    right = session.var(LivePerson, subtypes=True)
    left_name = left.field(LIVE_PERSON_NAME)
    right_name = right.field(LIVE_PERSON_NAME)
    pairs = (
        session.query(left, right)
        .allow_cross_join(left, right)
        .where(left_name.neq(right_name))
        .rows(
            limit=5,
            offset=2,
            order_by=(left_name.asc(), right_name.asc()),
        )
    )
    assert [(_name(first), _name(second)) for first, second in pairs] == [
        ("Alice", "Dave"),
        ("Bob", "Alice"),
        ("Bob", "Carol"),
        ("Bob", "Dave"),
        ("Carol", "Alice"),
    ]


@pytest.mark.integration
def test_typed_hidden_witnesses_deduplicate_selected_identity(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    person = session.var(LivePerson, subtypes=True)
    employment = session.var(LiveEmployment)
    company = session.var(LiveCompany)
    rows = (
        session.query(person)
        .match(employment, company)
        .where(
            employment.role(LiveEmployment.employee).connects(person),
            employment.role(LiveEmployment.employer).connects(company),
            person.field(LIVE_PERSON_NAME).eq(LiveName("Alice")),
        )
        .rows(limit=10)
    )
    assert len(rows) == 1
    assert _name(rows[0]) == "Alice"


@pytest.mark.integration
def test_typed_bounded_reachability_depths_cycles_and_shared_subtrees(
    typed_query_graph,
) -> None:
    session = QuerySession(typed_query_graph)
    source = session.var(LivePerson, subtypes=True)
    target = session.var(LivePerson, subtypes=True)
    source_name = source.field(LIVE_PERSON_NAME)
    target_name = target.field(LIVE_PERSON_NAME)

    def people(
        min_depth: int,
        max_depth: int,
    ) -> list[tuple[str, type[LivePerson], int | None, frozenset[str]]]:
        path = session.reachable(
            source,
            target,
            LiveComposition,
            LiveComposition.parent,
            LiveComposition.child,
            min_depth=min_depth,
            max_depth=max_depth,
        )
        rows = (
            session.query(target)
            .match(source)
            .where(source_name.eq(LiveName("Alice")), path)
            .rows(limit=10, order_by=(target_name.asc(),))
        )
        return [
            (
                _name(value),
                type(value),
                None if value.ranking is None else value.ranking.value,
                frozenset(label.value for label in value.labels),
            )
            for value in rows
        ]

    alice = ("Alice", LiveEmployee, 2, frozenset({"blue", "red"}))
    bob = ("Bob", LivePerson, 1, frozenset({"blue"}))
    carol = ("Carol", LivePerson, None, frozenset({"green"}))
    dave = ("Dave", LiveEmployee, 3, frozenset({"red"}))

    assert people(0, 0) == [alice]
    assert people(1, 1) == [bob, carol]
    assert people(2, 2) == [dave]
    assert people(3, 3) == [alice]
    assert people(1, 2) == [bob, carol, dave]
    assert people(0, 3) == [alice, bob, carol, dave]


@pytest.mark.integration
def test_typed_exact_one_scans_past_duplicate_hidden_witnesses(
    typed_query_graph,
) -> None:
    session = QuerySession(typed_query_graph)
    person = session.var(LivePerson, subtypes=True)
    employment = session.var(LiveEmployment)
    company = session.var(LiveCompany)
    query = (
        session.query(person)
        .match(employment, company)
        .where(
            employment.role(LiveEmployment.employee).connects(person),
            employment.role(LiveEmployment.employer).connects(company),
        )
    )

    # Alice has multiple hidden employment witnesses, but cardinality belongs
    # to the distinct selected person identity rather than provider rows.
    rows = query.rows(
        limit=10,
        order_by=(person.field(LIVE_PERSON_NAME).asc(),),
    )
    assert [_name(value) for value in rows] == ["Alice", "Bob", "Dave"]
    with pytest.raises(type_bridge_core.MatchRequestError) as multiple:
        query.one()
    assert multiple.value.code == "not_unique"


@pytest.mark.integration
def test_typed_five_slot_cycle_and_complete_relation_hydration(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    first_person = session.var(LivePerson, subtypes=True)
    first_employment = session.var(LiveEmployment)
    company = session.var(LiveCompany)
    second_employment = session.var(LiveEmployment)
    second_person = session.var(LivePerson, subtypes=True)
    row = (
        session.query(
            first_person,
            first_employment,
            company,
            second_employment,
            second_person,
        )
        .where(
            first_employment.role(LiveEmployment.employee).connects(first_person),
            first_employment.role(LiveEmployment.employer).connects(company),
            first_employment.role(LiveEmployment.reviewers).connects(second_person),
            second_employment.role(LiveEmployment.employee).connects(second_person),
            second_employment.role(LiveEmployment.employer).connects(company),
            first_person.field(LIVE_PERSON_NAME).eq(LiveName("Alice")),
        )
        .one()
    )
    person, employment, employer, peer_employment, peer = row
    assert (_name(person), employment.external_code.value) == ("Alice", "E-02")
    assert (_name(employer), peer_employment.external_code.value, _name(peer)) == (
        "Acme",
        "E-04",
        "Dave",
    )
    assert type(employment.employee) is LiveEmployee
    assert _name(employment.employee) == "Alice"
    assert _name(employment.employer) == "Acme"
    reviewers = _people(employment.reviewers)
    assert [_name(reviewer) for reviewer in reviewers] == ["Dave"]


@pytest.mark.integration
def test_typed_borrowed_read_context_survives_multiple_terminals(typed_query_graph) -> None:
    with typed_query_graph.transaction("read") as transaction:
        session = QuerySession(transaction)
        person = session.var(LivePerson, subtypes=True)
        name = person.field(LIVE_PERSON_NAME)
        query = session.query(person)

        first = query.rows(limit=2, order_by=(name.asc(),))
        second = query.rows(limit=2, offset=2, order_by=(name.asc(),))

        assert [_name(value) for value in first] == ["Alice", "Bob"]
        assert [_name(value) for value in second] == ["Carol", "Dave"]


@pytest.mark.integration
def test_typed_pages_count_exists_and_named_collections(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    person = session.var(LivePerson, subtypes=True)
    employment = session.var(LiveEmployment)
    company = session.var(LiveCompany)
    person_name = person.field(LIVE_PERSON_NAME)
    employment_code = employment.field(LIVE_EMPLOYMENT_CODE)
    company_name = company.field(LIVE_COMPANY_NAME)
    employments = employment.collect().order_by(employment_code.asc())
    companies = company.collect().distinct().order_by(company_name.asc())
    predicates = (
        employment.role(LiveEmployment.employee).connects(person),
        employment.role(LiveEmployment.employer).connects(company),
    )
    positional = session.query(person, employments, companies).where(*predicates)

    full = positional.page_by(
        person,
        limit=10,
        order_by=(person_name.asc(),),
        include_total=True,
    )
    assert full.offset == 0
    assert full.limit == 10
    assert full.total == 3
    assert isinstance(full.items, tuple)
    assert [_name(row[0]) for row in full.items] == ["Alice", "Bob", "Dave"]
    assert [item.external_code.value for item in full.items[0][1]] == ["E-01", "E-02"]
    assert [_name(item) for item in full.items[0][2]] == ["Acme", "Globex"]
    assert isinstance(full.items[0][1], tuple)
    assert isinstance(full.items[0][2], tuple)
    with pytest.raises(FrozenInstanceError):
        setattr(full, "total", 4)

    partial = positional.page_by(
        person,
        limit=1,
        offset=1,
        order_by=(person_name.asc(),),
    )
    assert partial.total is None
    assert partial.offset == 1
    assert [_name(row[0]) for row in partial.items] == ["Bob"]

    empty = positional.page_by(
        person,
        limit=2,
        offset=3,
        order_by=(person_name.asc(),),
        include_total=True,
    )
    assert empty.items == ()
    assert empty.total == 3
    assert positional.count_by(person) == 3
    assert positional.exists_by(person) is True

    no_match = positional.where(person_name.eq(LiveName("Nobody")))
    no_match_page = no_match.page_by(person, limit=2, include_total=True)
    assert no_match_page.items == ()
    assert no_match_page.total == 0
    assert no_match.count_by(person) == 0
    assert no_match.exists_by(person) is False

    labels = person.field(LIVE_PERSON_LABELS)
    multi_valued = positional.where(labels.contains(LiveTag("e"))).page_by(
        person,
        limit=2,
        order_by=(person_name.asc(),),
        include_total=True,
    )
    assert [_name(row[0]) for row in multi_valued.items] == ["Alice", "Bob"]
    assert multi_valued.total == 3
    assert [employment.external_code.value for employment in multi_valued.items[0][1]] == [
        "E-01",
        "E-01",
        "E-02",
        "E-02",
    ]

    alice = person_name.eq(LiveName("Alice"))
    overlapping_or = positional.where(alice | alice).page_by(
        person,
        limit=2,
        include_total=True,
    )
    assert [_name(row[0]) for row in overlapping_or.items] == ["Alice"]
    assert overlapping_or.total == 1

    named = session.query_as(
        LivePersonWorkPage,
        person=person,
        employments=employments,
        companies=companies,
    ).where(*predicates)
    named_page = named.page_by(
        person,
        limit=1,
        order_by=(person_name.asc(),),
        include_total=True,
    )
    assert named_page.total == 3
    assert type(named_page.items[0]) is LivePersonWorkPage
    assert _name(named_page.items[0].person) == "Alice"
    assert [item.external_code.value for item in named_page.items[0].employments] == [
        "E-01",
        "E-02",
    ]
    assert [_name(item) for item in named_page.items[0].companies] == ["Acme", "Globex"]
    with pytest.raises(FrozenInstanceError):
        setattr(named_page.items[0], "person", person)


@pytest.mark.integration
def test_typed_page_collection_multiplicity_distinct_and_relation_roles(
    typed_query_graph,
) -> None:
    session = QuerySession(typed_query_graph)
    company = session.var(LiveCompany)
    employment = session.var(LiveEmployment)
    reviewer = session.var(LivePerson, subtypes=True)
    company_name = company.field(LIVE_COMPANY_NAME)
    reviewer_name = reviewer.field(LIVE_PERSON_NAME)
    reviewers = reviewer.collect().order_by(reviewer_name.asc())
    predicates = (
        employment.role(LiveEmployment.employer).connects(company),
        employment.role(LiveEmployment.reviewers).connects(reviewer),
    )

    multiplicity = (
        session.query(company, reviewers)
        .match(employment)
        .where(*predicates)
        .page_by(company, limit=10, order_by=(company_name.asc(),))
    )
    assert [_name(row[0]) for row in multiplicity.items] == ["Acme", "Globex"]
    assert [_name(value) for value in multiplicity.items[0][1]] == [
        "Alice",
        "Alice",
        "Dave",
    ]

    distinct = (
        session.query(company, reviewers.distinct())
        .match(employment)
        .where(*predicates)
        .page_by(company, limit=10, order_by=(company_name.asc(),))
    )
    assert [_name(value) for value in distinct.items[0][1]] == ["Alice", "Dave"]

    relation = session.var(LiveEmployment)
    relation_page = session.query(relation).page_by(
        relation,
        limit=2,
        order_by=(relation.field(LIVE_EMPLOYMENT_CODE).asc(),),
        include_total=True,
    )
    assert relation_page.total == 4
    assert [value.external_code.value for value in relation_page.items] == [
        "E-01",
        "E-02",
    ]
    first = relation_page.items[0]
    assert type(first) is LiveEmployment
    assert type(first.employee) is LiveEmployee
    assert _name(first.employee) == "Alice"
    assert _name(first.employer) == "Globex"
    assert [_name(value) for value in _people(first.reviewers)] == ["Bob", "Dave"]

    second_relation_page = session.query(relation).page_by(
        relation,
        limit=2,
        offset=2,
        order_by=(relation.field(LIVE_EMPLOYMENT_CODE).asc(),),
    )
    assert [value.external_code.value for value in second_relation_page.items] == [
        "E-03",
        "E-04",
    ]


@pytest.mark.integration
def test_typed_borrowed_context_reuses_page_count_and_exists(typed_query_graph) -> None:
    with typed_query_graph.transaction("read") as transaction:
        session = QuerySession(transaction)
        person = session.var(LivePerson, subtypes=True)
        name = person.field(LIVE_PERSON_NAME)
        query = session.query(person)

        first = query.page_by(person, limit=2, order_by=(name.asc(),), include_total=True)
        assert [_name(value) for value in first.items] == ["Alice", "Bob"]
        assert first.total == 4
        assert query.count_by(person) == 4
        assert query.exists_by(person) is True

        second = query.page_by(person, limit=2, offset=2, order_by=(name.asc(),))
        assert [_name(value) for value in second.items] == ["Carol", "Dave"]
        assert query.rows(limit=1, order_by=(name.asc(),))[0].display_name.value == "Alice"


@pytest.mark.integration
def test_typed_nullable_order_fails_closed(typed_query_graph) -> None:
    session = QuerySession(typed_query_graph)
    person = session.var(LivePerson, subtypes=True)
    ranking = person.field(LIVE_PERSON_RANK)

    with pytest.raises(type_bridge_core.MatchRequestError) as raised:
        session.query(person).page_by(
            person,
            limit=2,
            order_by=(ranking.asc(),),
        )
    assert raised.value.category == "unsupported_capability"
    assert raised.value.code == "nullable_order_field_unsupported"
