"""Unit tests for the lifecycle hook system."""

from __future__ import annotations

import logging
from typing import Any, cast

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags
from type_bridge.crud.hooks import CrudEvent, HookCancelled, HookRunner
from type_bridge.crud.typedb_manager import TypeDBManager
from type_bridge.session import Database
from type_bridge.typedb_driver import TransactionType

# ---------------------------------------------------------------------------
# Test helpers
# ---------------------------------------------------------------------------


class _RecordingTypeDBManager(TypeDBManager):
    """TypeDBManager that records executed queries instead of hitting TypeDB."""

    def __init__(self, model_class: type):
        super().__init__(cast(Database, object()), model_class)
        self.queries: list[str] = []

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        self.queries.append(query)
        return []


class _RecordingHook:
    """Hook that records all calls for testing."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, type, Any]] = []

    def pre_insert(self, sender: type, instance: Any) -> None:
        self.calls.append(("pre_insert", sender, instance))

    def post_insert(self, sender: type, instance: Any) -> None:
        self.calls.append(("post_insert", sender, instance))

    def pre_update(self, sender: type, instance: Any) -> None:
        self.calls.append(("pre_update", sender, instance))

    def post_update(self, sender: type, instance: Any) -> None:
        self.calls.append(("post_update", sender, instance))

    def pre_delete(self, sender: type, instance: Any) -> None:
        self.calls.append(("pre_delete", sender, instance))

    def post_delete(self, sender: type, instance: Any) -> None:
        self.calls.append(("post_delete", sender, instance))

    def pre_put(self, sender: type, instance: Any) -> None:
        self.calls.append(("pre_put", sender, instance))

    def post_put(self, sender: type, instance: Any) -> None:
        self.calls.append(("post_put", sender, instance))


class _Name(String):
    pass


class _Age(Integer):
    pass


class _Person(Entity):
    flags = TypeFlags(name="hook_test_person")
    name: _Name = Flag(Key)
    age: _Age | None = None


# ============================================================================
# HookRunner isolation tests
# ============================================================================


class TestHookRunnerRegistration:
    def test_no_hooks_initially(self):
        runner = HookRunner()
        assert runner.has_hooks is False

    def test_add_hook(self):
        runner = HookRunner()
        hook = _RecordingHook()
        runner.add(hook)
        assert runner.has_hooks is True

    def test_remove_hook(self):
        runner = HookRunner()
        hook = _RecordingHook()
        runner.add(hook)
        runner.remove(hook)
        assert runner.has_hooks is False

    def test_remove_nonexistent_hook_raises(self):
        runner = HookRunner()
        with pytest.raises(ValueError):
            runner.remove(_RecordingHook())


class TestHookRunnerPreHooks:
    def test_pre_hook_called(self):
        runner = HookRunner()
        hook = _RecordingHook()
        runner.add(hook)

        alice = _Person(name=_Name("Alice"))
        runner.run_pre(CrudEvent.PRE_INSERT, _Person, alice)

        assert len(hook.calls) == 1
        assert hook.calls[0] == ("pre_insert", _Person, alice)

    def test_pre_hooks_run_in_registration_order(self):
        runner = HookRunner()
        order: list[str] = []

        class HookA:
            def pre_insert(self, sender: type, instance: Any) -> None:
                order.append("A")

        class HookB:
            def pre_insert(self, sender: type, instance: Any) -> None:
                order.append("B")

        runner.add(HookA())
        runner.add(HookB())

        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("Alice")))
        assert order == ["A", "B"]

    def test_pre_hook_cancellation(self):
        runner = HookRunner()

        class CancellingHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                raise HookCancelled("not allowed")

        runner.add(CancellingHook())

        with pytest.raises(HookCancelled, match="not allowed") as exc_info:
            runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("Alice")))

        assert exc_info.value.event == CrudEvent.PRE_INSERT

    def test_cancellation_stops_subsequent_hooks(self):
        runner = HookRunner()
        called = []

        class CancellingHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                called.append("cancel")
                raise HookCancelled("stop")

        class SecondHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                called.append("second")

        runner.add(CancellingHook())
        runner.add(SecondHook())

        with pytest.raises(HookCancelled):
            runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("Alice")))

        assert called == ["cancel"]

    def test_pre_hook_can_mutate_instance(self):
        runner = HookRunner()

        class MutatingHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                instance.age = _Age(99)

        runner.add(MutatingHook())

        alice = _Person(name=_Name("Alice"))
        runner.run_pre(CrudEvent.PRE_INSERT, _Person, alice)

        assert alice.age is not None
        assert alice.age.value == 99


class TestHookRunnerPostHooks:
    def test_post_hook_called(self):
        runner = HookRunner()
        hook = _RecordingHook()
        runner.add(hook)

        alice = _Person(name=_Name("Alice"))
        runner.run_post(CrudEvent.POST_INSERT, _Person, alice)

        assert len(hook.calls) == 1
        assert hook.calls[0] == ("post_insert", _Person, alice)

    def test_post_hooks_run_in_reverse_order(self):
        runner = HookRunner()
        order: list[str] = []

        class HookA:
            def post_insert(self, sender: type, instance: Any) -> None:
                order.append("A")

        class HookB:
            def post_insert(self, sender: type, instance: Any) -> None:
                order.append("B")

        runner.add(HookA())
        runner.add(HookB())

        runner.run_post(CrudEvent.POST_INSERT, _Person, _Person(name=_Name("Alice")))
        assert order == ["B", "A"]

    def test_post_hook_error_logged_not_propagated(self, caplog: pytest.LogCaptureFixture):
        runner = HookRunner()

        class FailingHook:
            def post_insert(self, sender: type, instance: Any) -> None:
                raise RuntimeError("boom")

        runner.add(FailingHook())

        with caplog.at_level(logging.ERROR, logger="type_bridge.crud.hooks"):
            runner.run_post(CrudEvent.POST_INSERT, _Person, _Person(name=_Name("Alice")))

        assert "boom" in caplog.text

    def test_post_hook_error_does_not_prevent_other_hooks(self, caplog: pytest.LogCaptureFixture):
        runner = HookRunner()
        called: list[str] = []

        class FailingHook:
            def post_insert(self, sender: type, instance: Any) -> None:
                called.append("failing")
                raise RuntimeError("boom")

        class GoodHook:
            def post_insert(self, sender: type, instance: Any) -> None:
                called.append("good")

        # Reverse order: GoodHook added first runs second, FailingHook added second runs first
        runner.add(GoodHook())
        runner.add(FailingHook())

        with caplog.at_level(logging.ERROR, logger="type_bridge.crud.hooks"):
            runner.run_post(CrudEvent.POST_INSERT, _Person, _Person(name=_Name("Alice")))

        # FailingHook runs first (reverse order), GoodHook second
        assert called == ["failing", "good"]


class TestHookRunnerShouldRun:
    def test_should_run_false_skips_hook(self):
        runner = HookRunner()
        called = False

        class FilteredHook:
            def should_run(self, event: CrudEvent, sender: type) -> bool:
                return False

            def pre_insert(self, sender: type, instance: Any) -> None:
                nonlocal called
                called = True

        runner.add(FilteredHook())
        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("Alice")))
        assert called is False

    def test_should_run_filters_by_event(self):
        runner = HookRunner()
        hook = _RecordingHook()

        class InsertOnlyHook(_RecordingHook):
            def should_run(self, event: CrudEvent, sender: type) -> bool:
                return event in (CrudEvent.PRE_INSERT, CrudEvent.POST_INSERT)

        filtered = InsertOnlyHook()
        runner.add(filtered)

        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("A")))
        runner.run_pre(CrudEvent.PRE_UPDATE, _Person, _Person(name=_Name("B")))

        assert len(filtered.calls) == 1
        assert filtered.calls[0][0] == "pre_insert"

    def test_should_run_filters_by_sender(self):
        runner = HookRunner()

        class AnotherName(String):
            pass

        class Cat(Entity):
            flags = TypeFlags(name="hook_test_cat")
            name: AnotherName = Flag(Key)

        class PersonOnlyHook(_RecordingHook):
            def should_run(self, event: CrudEvent, sender: type) -> bool:
                return sender is _Person

        filtered = PersonOnlyHook()
        runner.add(filtered)

        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("A")))
        runner.run_pre(CrudEvent.PRE_INSERT, Cat, Cat(name=AnotherName("Whiskers")))

        assert len(filtered.calls) == 1

    def test_hook_without_should_run_always_runs(self):
        runner = HookRunner()
        hook = _RecordingHook()
        runner.add(hook)

        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("A")))
        runner.run_pre(CrudEvent.PRE_UPDATE, _Person, _Person(name=_Name("B")))
        runner.run_pre(CrudEvent.PRE_DELETE, _Person, _Person(name=_Name("C")))

        assert len(hook.calls) == 3


class TestHookRunnerEdgeCases:
    def test_empty_hook_class_is_harmless(self):
        runner = HookRunner()
        runner.add(object())  # no hook methods at all

        # Should not raise
        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("A")))
        runner.run_post(CrudEvent.POST_INSERT, _Person, _Person(name=_Name("A")))

    def test_partial_hook_only_post_insert(self):
        runner = HookRunner()
        called = False

        class PostOnlyHook:
            def post_insert(self, sender: type, instance: Any) -> None:
                nonlocal called
                called = True

        runner.add(PostOnlyHook())

        # pre_insert should not call post_insert
        runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("A")))
        assert called is False

        # post_insert should
        runner.run_post(CrudEvent.POST_INSERT, _Person, _Person(name=_Name("A")))
        assert called is True


# ============================================================================
# TypeDBManager hook integration tests
# ============================================================================


class TestManagerHookRegistration:
    def test_add_hook_returns_self(self):
        mgr = _RecordingTypeDBManager(_Person)
        result = mgr.add_hook(_RecordingHook())
        assert result is mgr

    def test_add_hook_chaining(self):
        mgr = _RecordingTypeDBManager(_Person)
        hook1 = _RecordingHook()
        hook2 = _RecordingHook()
        result = mgr.add_hook(hook1).add_hook(hook2)
        assert result is mgr

    def test_remove_hook(self):
        mgr = _RecordingTypeDBManager(_Person)
        hook = _RecordingHook()
        mgr.add_hook(hook)
        mgr.remove_hook(hook)
        # No hooks, insert should not trigger any hook calls
        alice = _Person(name=_Name("Alice"))
        mgr.insert(alice)
        assert hook.calls == []


class TestManagerInsertHooks:
    def test_insert_fires_pre_and_post(self):
        hook = _RecordingHook()
        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(hook)

        alice = _Person(name=_Name("Alice"))
        mgr.insert(alice)

        assert len(hook.calls) == 2
        assert hook.calls[0] == ("pre_insert", _Person, alice)
        assert hook.calls[1] == ("post_insert", _Person, alice)

    def test_insert_pre_hook_cancellation_prevents_query(self):
        class CancelHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                raise HookCancelled("nope")

        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(CancelHook())

        with pytest.raises(HookCancelled, match="nope"):
            mgr.insert(_Person(name=_Name("Alice")))

        # No queries should have been executed
        assert mgr.queries == []


class TestManagerUpdateHooks:
    def test_update_fires_pre_and_post(self):
        hook = _RecordingHook()
        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(hook)

        alice = _Person(name=_Name("Alice"))
        mgr.update(alice)

        assert len(hook.calls) == 2
        assert hook.calls[0] == ("pre_update", _Person, alice)
        assert hook.calls[1] == ("post_update", _Person, alice)


class TestManagerDeleteHooks:
    def test_delete_fires_pre_and_post(self):
        hook = _RecordingHook()
        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(hook)

        alice = _Person(name=_Name("Alice"))
        mgr.delete(alice)

        assert len(hook.calls) == 2
        assert hook.calls[0] == ("pre_delete", _Person, alice)
        assert hook.calls[1] == ("post_delete", _Person, alice)


class TestManagerPutHooks:
    def test_put_fires_pre_and_post(self):
        hook = _RecordingHook()
        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(hook)

        alice = _Person(name=_Name("Alice"))
        mgr.put(alice)

        assert len(hook.calls) == 2
        assert hook.calls[0] == ("pre_put", _Person, alice)
        assert hook.calls[1] == ("post_put", _Person, alice)


class TestManagerBatchHooks:
    def test_insert_many_fires_hooks_per_instance(self):
        hook = _RecordingHook()
        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(hook)

        alice = _Person(name=_Name("Alice"))
        bob = _Person(name=_Name("Bob"))
        mgr.insert_many([alice, bob])

        # Pre-hooks in order, then post-hooks in order
        pre_calls = [c for c in hook.calls if c[0] == "pre_insert"]
        post_calls = [c for c in hook.calls if c[0] == "post_insert"]

        assert len(pre_calls) == 2
        assert pre_calls[0][2] is alice
        assert pre_calls[1][2] is bob

        assert len(post_calls) == 2
        assert post_calls[0][2] is alice
        assert post_calls[1][2] is bob

    def test_insert_many_cancellation_prevents_all(self):
        call_count = 0

        class CancelOnSecond:
            def pre_insert(self, sender: type, instance: Any) -> None:
                nonlocal call_count
                call_count += 1
                if call_count == 2:
                    raise HookCancelled("stop at second")

        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(CancelOnSecond())

        with pytest.raises(HookCancelled, match="stop at second"):
            mgr.insert_many([_Person(name=_Name("A")), _Person(name=_Name("B"))])

        # No queries should have been executed (pre-hooks run before any DB op)
        assert mgr.queries == []

    def test_update_many_fires_hooks_per_instance(self):
        """update_many delegates to update(), which fires hooks individually."""
        hook = _RecordingHook()
        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(hook)

        alice = _Person(name=_Name("Alice"))
        bob = _Person(name=_Name("Bob"))
        mgr.update_many([alice, bob])

        pre_calls = [c for c in hook.calls if c[0] == "pre_update"]
        post_calls = [c for c in hook.calls if c[0] == "post_update"]
        assert len(pre_calls) == 2
        assert len(post_calls) == 2


class TestManagerPreHookMutation:
    def test_pre_insert_mutation_before_query(self):
        """Pre-hook mutation should affect the instance before query generation."""

        class SetAgeHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                instance.age = _Age(42)

        mgr = _RecordingTypeDBManager(_Person)
        mgr.add_hook(SetAgeHook())

        alice = _Person(name=_Name("Alice"))
        assert alice.age is None

        mgr.insert(alice)
        assert alice.age is not None
        assert alice.age.value == 42


class TestZeroOverhead:
    def test_no_hooks_has_hooks_false(self):
        mgr = _RecordingTypeDBManager(_Person)
        assert mgr._hook_runner.has_hooks is False

    def test_insert_works_without_hooks(self):
        """Insert should work normally when no hooks are registered."""
        mgr = _RecordingTypeDBManager(_Person)
        alice = _Person(name=_Name("Alice"))
        result = mgr.insert(alice)
        assert result is alice
        assert len(mgr.queries) > 0


# ============================================================================
# CrudEvent and HookCancelled tests
# ============================================================================


class TestCrudEvent:
    def test_all_events_have_correct_values(self):
        assert CrudEvent.PRE_INSERT.value == "pre_insert"
        assert CrudEvent.POST_INSERT.value == "post_insert"
        assert CrudEvent.PRE_UPDATE.value == "pre_update"
        assert CrudEvent.POST_UPDATE.value == "post_update"
        assert CrudEvent.PRE_DELETE.value == "pre_delete"
        assert CrudEvent.POST_DELETE.value == "post_delete"
        assert CrudEvent.PRE_PUT.value == "pre_put"
        assert CrudEvent.POST_PUT.value == "post_put"

    def test_eight_events(self):
        assert len(CrudEvent) == 8


class TestHookCancelled:
    def test_reason(self):
        exc = HookCancelled("bad data")
        assert exc.reason == "bad data"
        assert str(exc) == "bad data"

    def test_context_attributes(self):
        hook = object()
        exc = HookCancelled("stop", event=CrudEvent.PRE_INSERT, hook=hook)
        assert exc.event == CrudEvent.PRE_INSERT
        assert exc.hook is hook

    def test_defaults(self):
        exc = HookCancelled()
        assert exc.reason == ""
        assert exc.event is None
        assert exc.hook is None

    def test_enriched_by_runner(self):
        runner = HookRunner()

        class MyHook:
            def pre_insert(self, sender: type, instance: Any) -> None:
                raise HookCancelled("no")

        my_hook = MyHook()
        runner.add(my_hook)

        with pytest.raises(HookCancelled) as exc_info:
            runner.run_pre(CrudEvent.PRE_INSERT, _Person, _Person(name=_Name("A")))

        assert exc_info.value.event == CrudEvent.PRE_INSERT
        assert exc_info.value.hook is my_hook
