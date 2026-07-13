#!/usr/bin/env python3
"""Assert the installed legacy Python package contract.

This file is copied out of the checkout by ``run_legacy_python_compat.py`` and
executed with ``python -I``. It intentionally uses only recording objects: no
TypeDB connection, query execution, or source-tree helper is allowed.
"""

from __future__ import annotations

import argparse
import importlib.metadata
import importlib.util
import json
import sys
from collections.abc import Iterable
from pathlib import Path
from types import ModuleType
from typing import Any


class CompatibilityError(AssertionError):
    """An installed package violated the recorded legacy contract."""


def require(condition: bool, message: str) -> None:
    """Raise a compatibility failure with a stable diagnostic."""
    if not condition:
        raise CompatibilityError(message)


def path_is_within(path: str | Path, root: str | Path) -> bool:
    """Return whether a resolved path is equal to or contained by root."""
    candidate = Path(path).expanduser().resolve()
    boundary = Path(root).expanduser().resolve()
    return candidate == boundary or boundary in candidate.parents


def source_leaks(paths: Iterable[str | Path], source_root: str | Path) -> list[str]:
    """Return sorted source-root paths from an import-path collection."""
    leaks: set[str] = set()
    for raw_path in paths:
        if not raw_path:
            continue
        try:
            path = Path(raw_path).expanduser().resolve()
        except OSError:
            continue
        if path_is_within(path, source_root):
            leaks.add(str(path))
    return sorted(leaks)


def module_paths(module: ModuleType) -> list[str]:
    """Collect all concrete import locations exposed by a module."""
    paths: list[str] = []
    module_file = getattr(module, "__file__", None)
    if module_file:
        paths.append(str(module_file))
    module_path = getattr(module, "__path__", None)
    if module_path is not None:
        paths.extend(str(path) for path in module_path)
    spec = getattr(module, "__spec__", None)
    locations = getattr(spec, "submodule_search_locations", None)
    if locations is not None:
        paths.extend(str(path) for path in locations)
    return paths


def distribution_record(name: str) -> dict[str, str]:
    """Return the installed distribution version and root location."""
    distribution = importlib.metadata.distribution(name)
    location = Path(str(distribution.locate_file(""))).resolve()
    return {"version": distribution.version, "location": str(location)}


def assert_no_source_leakage(source_root: Path) -> dict[str, Any]:
    """Fail if import state or installed distributions resolve into checkout."""
    observed_paths: list[str] = [entry for entry in sys.path if entry]
    modules: dict[str, list[str]] = {}
    for name, module in sorted(sys.modules.items()):
        if name != "type_bridge" and not name.startswith(("type_bridge.", "type_bridge_core")):
            continue
        paths = module_paths(module)
        modules[name] = paths
        observed_paths.extend(paths)

    distributions = {
        name: distribution_record(name) for name in ("type-bridge", "type-bridge-core")
    }
    observed_paths.extend(record["location"] for record in distributions.values())
    leaks = source_leaks(observed_paths, source_root)
    require(not leaks, f"source-tree import leakage detected: {leaks}")
    return {
        "distributions": distributions,
        "modules": modules,
        "source_root": str(source_root),
    }


def define_models() -> tuple[type[Any], type[Any], type[Any], type[Any], type[Any]]:
    """Define a small Pydantic model graph using only installed public APIs."""
    from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags

    # `get_type_hints` resolves the postponed `Role[...]` annotation from this
    # module namespace while Relation scans the class during construction.
    globals()["Role"] = Role

    class LegacyCompatName(String):
        pass

    class LegacyCompatAge(Integer):
        pass

    # This probe uses postponed annotations and defines its disposable models
    # inside a function. Publish the attribute classes in the module namespace
    # before Pydantic resolves the entity annotations, matching ordinary
    # module-level user models.
    globals()["LegacyCompatName"] = LegacyCompatName
    globals()["LegacyCompatAge"] = LegacyCompatAge

    class LegacyCompatPerson(Entity):
        flags = TypeFlags(name="legacy-compat-person")
        name: LegacyCompatName = Flag(Key)
        age: LegacyCompatAge

    class LegacyCompatCompany(Entity):
        flags = TypeFlags(name="legacy-compat-company")
        name: LegacyCompatName = Flag(Key)

    globals()["LegacyCompatPerson"] = LegacyCompatPerson
    globals()["LegacyCompatCompany"] = LegacyCompatCompany

    class LegacyCompatEmployment(Relation):
        flags = TypeFlags(name="legacy-compat-employment")
        employee: Role[LegacyCompatPerson] = Role("employee", LegacyCompatPerson)
        employer: Role[LegacyCompatCompany] = Role("employer", LegacyCompatCompany)

    return (
        LegacyCompatName,
        LegacyCompatAge,
        LegacyCompatPerson,
        LegacyCompatCompany,
        LegacyCompatEmployment,
    )


def define_polymorphic_models() -> tuple[type[Any], type[Any], type[Any]]:
    """Define an abstract entity and two concrete legacy manager subtypes."""
    from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags

    class LegacyCompatArtifactName(String):
        pass

    class LegacyCompatPriority(Integer):
        pass

    class LegacyCompatCategory(String):
        pass

    globals()["LegacyCompatArtifactName"] = LegacyCompatArtifactName
    globals()["LegacyCompatPriority"] = LegacyCompatPriority
    globals()["LegacyCompatCategory"] = LegacyCompatCategory

    class LegacyCompatArtifact(Entity):
        flags = TypeFlags(name="legacy-compat-artifact", abstract=True)
        name: LegacyCompatArtifactName = Flag(Key)

    globals()["LegacyCompatArtifact"] = LegacyCompatArtifact

    class LegacyCompatStory(LegacyCompatArtifact):
        flags = TypeFlags(name="legacy-compat-story")
        priority: LegacyCompatPriority

    class LegacyCompatAspect(LegacyCompatArtifact):
        flags = TypeFlags(name="legacy-compat-aspect")
        category: LegacyCompatCategory

    return LegacyCompatArtifact, LegacyCompatStory, LegacyCompatAspect


def probe_raw_query(person_type: type[Any]) -> dict[str, Any]:
    """Pin package-root raw builder identity, mutation, and exact output."""
    import type_bridge
    from type_bridge import Query, QueryBuilder
    from type_bridge.query import Query as ModuleQuery
    from type_bridge.query import QueryBuilder as ModuleQueryBuilder

    require(Query is ModuleQuery, "type_bridge.Query no longer re-exports raw Query")
    require(
        QueryBuilder is ModuleQueryBuilder,
        "type_bridge.QueryBuilder no longer re-exports raw QueryBuilder",
    )
    require("Query" in type_bridge.__all__, "Query is missing from type_bridge.__all__")
    require("QueryBuilder" in type_bridge.__all__, "QueryBuilder is missing from __all__")

    query = Query()
    for operation in (
        lambda: query.match("$p isa person;"),
        lambda: query.match("$p has name $n"),
        lambda: query.fetch("$p"),
        lambda: query.sort("$n", "desc"),
        lambda: query.offset(2),
        lambda: query.limit(5),
    ):
        require(operation() is query, "raw Query builder operation stopped mutating in place")

    expected = "\n".join(
        (
            "match",
            "$p isa person;",
            "$p has name $n;",
            "sort $n desc;",
            "offset 2;",
            "limit 5;",
            "fetch {",
            '  "p": $p.*',
            "};",
        )
    )
    built = query.build()
    require(built == expected, f"raw Query build output drifted: {built!r}")
    require(str(query) == expected, "raw Query.__str__ no longer delegates to build")

    helper = QueryBuilder.match_entity(person_type, "$person", name="Alice")
    require(isinstance(helper, Query), "QueryBuilder.match_entity no longer returns raw Query")
    helper.fetch("$person")
    expected_helper = "\n".join(
        (
            "match",
            '$person isa legacy-compat-person, has LegacyCompatName "Alice";',
            "fetch {",
            '  "person": $person.*',
            "};",
        )
    )
    helper_built = helper.build()
    require(helper_built == expected_helper, f"QueryBuilder output drifted: {helper_built!r}")
    return {"raw_query": built, "query_builder": helper_built}


def probe_descriptors(
    name_type: type[Any],
    age_type: type[Any],
    person_type: type[Any],
    company_type: type[Any],
    employment_type: type[Any],
) -> dict[str, Any]:
    """Pin Pydantic descriptor class access versus instance values."""
    import type_bridge.fields as public_fields
    from type_bridge.fields import FieldDescriptor, FieldRef, NumericFieldRef, StringFieldRef
    from type_bridge.fields.role import RolePlayerStringFieldRef, RoleRef

    legacy_generics: tuple[tuple[str, object, type[Any]], ...] = (
        ("FieldRef", FieldRef, name_type),
        ("StringFieldRef", StringFieldRef, name_type),
        ("NumericFieldRef", NumericFieldRef, age_type),
        ("FieldDescriptor", FieldDescriptor, name_type),
        ("RoleRef", RoleRef, person_type),
    )
    for name, generic, argument in legacy_generics:
        subscribe = getattr(generic, "__class_getitem__", None)
        if not callable(subscribe):
            raise CompatibilityError(f"legacy generic {name} is not subscriptable")
        try:
            subscribe(argument)
        except TypeError as error:
            raise CompatibilityError(
                f"legacy one-parameter {name}[T] spelling failed: {error}"
            ) from error
    require(
        "OrderedFieldRef" not in public_fields.__all__
        and not hasattr(public_fields, "OrderedFieldRef"),
        "typed-only OrderedFieldRef leaked through type_bridge.fields",
    )

    name_ref = person_type.name
    age_ref = person_type.age
    require(isinstance(name_ref, StringFieldRef), "class string field is not StringFieldRef")
    require(isinstance(age_ref, NumericFieldRef), "class numeric field is not NumericFieldRef")
    require(name_ref.field_name == "name", "class field reference lost field name")
    require(name_ref.attr_type is name_type, "class field reference lost attribute type")
    require(name_ref.entity_type is person_type, "class field reference lost owner type")

    person = person_type(name="Alice", age=31)
    company = company_type(name="Acme")
    require(isinstance(person.name, name_type), "instance field is not wrapped attribute value")
    require(person.name.value == "Alice", "instance field value drifted")
    require(isinstance(person.age, age_type), "instance numeric field is not wrapped value")
    require(person.age.value == 31, "instance numeric field value drifted")

    role_ref = employment_type.employee
    require(isinstance(role_ref, RoleRef), "class role is not RoleRef")
    require(role_ref.role_name == "employee", "class role reference lost role name")
    require(role_ref.player_types == (person_type,), "class role lost player type")
    nested_ref = employment_type.employee.name
    require(
        isinstance(nested_ref, RolePlayerStringFieldRef),
        "class role-player field is not RolePlayerStringFieldRef",
    )

    employment = employment_type(employee=person, employer=company)
    require(employment.employee is person, "instance role did not return player value")
    require(employment.employer is company, "instance role did not return player value")
    require(isinstance(employment_type.employee, RoleRef), "instance access damaged class role")
    return {
        "field_ref": type(name_ref).__name__,
        "field_value": type(person.name).__name__,
        "role_ref": type(role_ref).__name__,
        "role_value": type(employment.employee).__name__,
        "legacy_generic_arity": [name for name, _, _ in legacy_generics],
    }


class RecordingNativeManager:
    """Record RustTypeDBQuery calls without opening a database connection."""

    def __init__(self, rows: list[Any]):
        self.rows = rows
        self.get_calls: list[dict[str, Any]] = []
        self.count_calls: list[dict[str, Any]] = []
        self.aggregate_calls: list[dict[str, Any]] = []
        self.group_calls: list[dict[str, Any]] = []

    def get_with_query(
        self,
        expressions: list[Any],
        sorts: list[Any],
        limit: int | None,
        offset: int | None,
    ) -> list[Any]:
        self.get_calls.append(
            {
                "expression_types": [type(item).__name__ for item in expressions],
                "sort_types": [type(item).__name__ for item in sorts],
                "limit": limit,
                "offset": offset,
            }
        )
        return list(self.rows)

    def count_with_query(self, expressions: list[Any]) -> int:
        self.count_calls.append({"expression_types": [type(item).__name__ for item in expressions]})
        return len(self.rows)

    def aggregate(self, aggregates: list[dict[str, Any]], filters: Any) -> list[dict[str, Any]]:
        self.aggregate_calls.append({"aggregates": aggregates, "filters": filters})
        return [{"$avg_age": {"value": 31.5}}]

    def group_by_aggregate(
        self,
        group_fields: list[str],
        aggregates: list[dict[str, Any]],
        filters: Any,
    ) -> list[dict[str, Any]]:
        self.group_calls.append(
            {"group_fields": group_fields, "aggregates": aggregates, "filters": filters}
        )
        return [{"$group0": {"value": "Alice"}, "$avg_age": {"value": 31.5}}]


class RecordingManagerFacade:
    """Minimal manager contract consumed by RustTypeDBQuery."""

    def __init__(self, model_class: type[Any], native: RecordingNativeManager):
        self.model_class = model_class
        self._kind = "entity"
        self._manager = native
        self.deleted: list[Any] = []
        self.updated: list[Any] = []

    def _hydrate_entity(self, row: Any) -> Any:
        return row

    def _hydrate_relation_rows(self, rows: list[dict[str, Any]]) -> list[Any]:
        return list(rows)

    def delete(self, instance: Any) -> Any:
        self.deleted.append(instance)
        return instance

    def update(self, instance: Any) -> Any:
        self.updated.append(instance)
        return instance


def probe_rust_query(person_type: type[Any]) -> dict[str, Any]:
    """Pin mutable RustTypeDBQuery specs and key recording terminals."""
    from type_bridge.crud.rust_manager import RustTypeDBQuery

    rows = [person_type(name="Alice", age=31), person_type(name="Bob", age=32)]
    native = RecordingNativeManager(rows)
    manager = RecordingManagerFacade(person_type, native)
    query = RustTypeDBQuery(manager, {})
    alias = query

    # Deliberately ignore every return. This is the current mutable contract.
    query.filter(name="Alice")
    query.limit(2)
    query.offset(1)
    query.order_by("-age")
    require(alias is query, "RustTypeDBQuery alias identity changed")

    require(query.all() == rows, "RustTypeDBQuery.all result drifted")
    initial_spec = native.get_calls[-1]
    require(initial_spec["limit"] == 2, "ignored limit return did not mutate query")
    require(initial_spec["offset"] == 1, "ignored offset return did not mutate query")
    require(len(initial_spec["expression_types"]) == 1, "filter spec count drifted")
    require(len(initial_spec["sort_types"]) == 1, "sort spec count drifted")

    require(query.execute() == rows, "RustTypeDBQuery.execute result drifted")
    require(query.first() is rows[0], "RustTypeDBQuery.first result drifted")
    require(native.get_calls[-1]["limit"] == 1, "first did not use temporary limit 1")
    query.execute()
    require(native.get_calls[-1]["limit"] == 2, "first did not restore prior limit")

    require(query.count() == 2, "RustTypeDBQuery.count result drifted")
    require(query.exists() is True, "RustTypeDBQuery.exists result drifted")
    require(len(native.count_calls) == 2, "count/exists recording call count drifted")

    aggregate = query.aggregate(person_type.age.avg())
    require(aggregate == {"avg_age": 31.5}, "aggregate result normalization drifted")
    aggregate_call = native.aggregate_calls[-1]
    require(
        aggregate_call["aggregates"]
        == [
            {
                "result_key": "avg_age",
                "function": "mean",
                "attr_name": "LegacyCompatAge",
            }
        ],
        "aggregate spec drifted",
    )
    require(aggregate_call["filters"] == {"name": "Alice"}, "aggregate filters drifted")

    grouped = query.group_by(person_type.name).aggregate(person_type.age.avg())
    require(grouped == {"Alice": {"avg_age": 31.5}}, "group-by result drifted")
    require(
        native.group_calls[-1]["group_fields"] == ["LegacyCompatName"],
        "group-by spec drifted",
    )

    updated = query.update_with(lambda instance: instance)
    require(updated == rows and manager.updated == rows, "update_with recording behavior drifted")
    deleted_count = query.delete()
    require(deleted_count == 2 and manager.deleted == rows, "delete recording behavior drifted")

    query.limit(3)
    alias.execute()
    require(native.get_calls[-1]["limit"] == 3, "ignored-return sibling aliasing drifted")

    lookup_query = RustTypeDBQuery(manager, {})
    lookup_query.filter(age__gte=30)
    lookup_query.order_by("-age")
    require(lookup_query.execute() == rows, "Django-style lookup result drifted")
    lookup_spec = native.get_calls[-1]
    require(
        lookup_spec["expression_types"] == ["DynamicExpr"],
        "Django-style __gte lookup no longer lowers to one native expression",
    )
    require(
        lookup_spec["sort_types"] == ["DynamicSort"],
        "legacy string ordering no longer lowers to one native sort",
    )
    return {
        "initial_spec": initial_spec,
        "lookup_spec": lookup_spec,
        "aggregate_spec": aggregate_call,
        "group_spec": native.group_calls[-1],
        "get_call_count": len(native.get_calls),
    }


def probe_polymorphic_manager(
    artifact_type: type[Any],
    story_type: type[Any],
    aspect_type: type[Any],
) -> dict[str, Any]:
    """Pin concrete-subtype hydration through the installed legacy manager."""
    from type_bridge.crud.rust_manager import RustTypeDBManager
    from type_bridge.session import Database

    rows = [
        {
            "_iid": "0x501",
            "_type": story_type.get_type_name(),
            "name": "Zulu Feature",
            "priority": 3,
        },
        {
            "_iid": "0x502",
            "_type": aspect_type.get_type_name(),
            "name": "Alpha Feature",
            "category": "Security",
        },
    ]
    native = RecordingNativeManager(rows)
    manager = RustTypeDBManager(Database(), artifact_type)
    manager._manager_instance = native

    results = manager.filter(name__contains="Feature").order_by("-name").execute()
    require(len(results) == 2, "polymorphic manager result count drifted")

    story, aspect = results
    require(type(story) is story_type, "base manager did not hydrate the story subtype")
    require(type(aspect) is aspect_type, "base manager did not hydrate the aspect subtype")
    require(story._iid == "0x501", "story subtype IID hydration drifted")
    require(aspect._iid == "0x502", "aspect subtype IID hydration drifted")
    require(story.name.value == "Zulu Feature", "inherited story field hydration drifted")
    require(story.priority.value == 3, "story-only field hydration drifted")
    require(aspect.name.value == "Alpha Feature", "inherited aspect field hydration drifted")
    require(aspect.category.value == "Security", "aspect-only field hydration drifted")

    lookup_spec = native.get_calls[-1]
    require(
        lookup_spec["expression_types"] == ["DynamicExpr"],
        "Django-style __contains lookup no longer lowers through the base manager",
    )
    require(
        lookup_spec["sort_types"] == ["DynamicSort"],
        "base-manager string ordering no longer lowers to one native sort",
    )
    return {
        "concrete_types": [type(item).__name__ for item in results],
        "lookup_spec": lookup_spec,
        "subtype_fields": {
            "story_priority": story.priority.value,
            "aspect_category": aspect.category.value,
        },
    }


def probe_typed_facade() -> dict[str, Any]:
    """Prove the typed subpath ships without replacing package-root Query."""
    import type_bridge
    import type_bridge.typed as typed
    from type_bridge.query import Query as RawQuery

    require(type_bridge.Query is RawQuery, "package-root Query no longer exposes the raw API")
    require(typed.Query is not RawQuery, "typed Query replaced or aliases package-root Query")
    require(typed.Query.__module__ == "type_bridge.typed.query", "typed Query export drifted")
    require(
        typed.QuerySession.__module__ == "type_bridge.typed.session",
        "typed QuerySession export drifted",
    )
    require("Query" in typed.__all__, "typed Query is missing from type_bridge.typed.__all__")
    require(
        "QuerySession" in typed.__all__,
        "typed QuerySession is missing from type_bridge.typed.__all__",
    )
    return {
        "module": typed.__name__,
        "query": f"{typed.Query.__module__}.{typed.Query.__name__}",
        "session": f"{typed.QuerySession.__module__}.{typed.QuerySession.__name__}",
        "root_query": f"{RawQuery.__module__}.{RawQuery.__name__}",
    }


def run(source_root: Path) -> dict[str, Any]:
    """Run every installed-package compatibility assertion."""
    import type_bridge_core

    import type_bridge

    require(type_bridge.Query.__module__ == "type_bridge.query", "root Query is no longer raw")
    require(
        importlib.util.find_spec("type_bridge.typed") is not None, "typed facade is not shipped"
    )
    initial_locations = assert_no_source_leakage(source_root)
    name_type, age_type, person_type, company_type, employment_type = define_models()
    artifact_type, story_type, aspect_type = define_polymorphic_models()
    report = {
        "status": "ok",
        "package_version": type_bridge.__version__,
        "native_module": type_bridge_core.__name__,
        "locations": initial_locations,
        "typed": probe_typed_facade(),
        "raw": probe_raw_query(person_type),
        "descriptors": probe_descriptors(
            name_type, age_type, person_type, company_type, employment_type
        ),
        "rust_query": probe_rust_query(person_type),
        "polymorphic_manager": probe_polymorphic_manager(
            artifact_type,
            story_type,
            aspect_type,
        ),
    }
    report["locations_after_probe"] = assert_no_source_leakage(source_root)
    return report


def main() -> int:
    """Parse the source boundary and print one machine-readable report."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(run(args.source_root.resolve()), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
