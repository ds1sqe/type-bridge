"""Staged document answers expose the shared engine's validated evidence."""

from type_bridge.plan import Documents, PlanAnswer, Rows


def test_document_answer_preserves_nested_evidence() -> None:
    documents: list[dict[str, object]] = [
        {"_iid": "0x01", "_type": "person", "attributes": {"name": ["Ada"]}}
    ]
    answer = Documents(documents)
    assert isinstance(answer, PlanAnswer)
    assert answer.documents == documents
    assert answer == Documents(documents)
    assert answer != Rows(documents)
    assert repr(answer).startswith("Documents(")
