"""Integration tests for multi-role player relations."""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


class DocumentId(String):
    pass


class EmailSubject(String):
    pass


class TraceLabel(String):
    pass


class Document(Entity):
    flags = TypeFlags(name="document")
    document_id: DocumentId = Flag(Key)


class Email(Entity):
    flags = TypeFlags(name="email")
    subject: EmailSubject = Flag(Key)


class Trace(Relation):
    """Relation where a single role can be played by multiple entity types."""

    flags = TypeFlags(name="trace")
    origin: Role[Document | Email] = Role.multi("origin", Document, Email)
    label: TraceLabel


@pytest.fixture(scope="function")
def db_with_multi_role_schema(clean_db):
    """Provide a database with multi-role schema defined."""
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Document, Email, Trace)
    schema_manager.sync_schema(force=True)
    yield clean_db


@pytest.mark.integration
@pytest.mark.order(25)
def test_insert_relation_with_multi_role_document(db_with_multi_role_schema):
    """Test inserting a relation where a Document plays the multi-role."""
    doc = Document(document_id=DocumentId("doc-001"))
    Document.manager(db_with_multi_role_schema).insert(doc)

    trace = Trace(origin=doc, label=TraceLabel("from-document"))
    Trace.manager(db_with_multi_role_schema).insert(trace)

    results = Trace.manager(db_with_multi_role_schema).all()
    assert len(results) == 1
    assert results[0].label.value == "from-document"


@pytest.mark.integration
@pytest.mark.order(26)
def test_insert_relation_with_multi_role_email(db_with_multi_role_schema):
    """Test inserting a relation where an Email plays the multi-role."""
    mail = Email(subject=EmailSubject("Important Notice"))
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace = Trace(origin=mail, label=TraceLabel("from-email"))
    Trace.manager(db_with_multi_role_schema).insert(trace)

    results = Trace.manager(db_with_multi_role_schema).all()
    assert len(results) == 1
    assert results[0].label.value == "from-email"


@pytest.mark.integration
@pytest.mark.order(27)
def test_insert_multiple_relations_with_different_role_players(db_with_multi_role_schema):
    """Test inserting multiple relations with different entity types for the same role."""
    doc = Document(document_id=DocumentId("doc-002"))
    mail = Email(subject=EmailSubject("Weekly Report"))

    Document.manager(db_with_multi_role_schema).insert(doc)
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace_from_doc = Trace(origin=doc, label=TraceLabel("trace-doc"))
    trace_from_mail = Trace(origin=mail, label=TraceLabel("trace-mail"))

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(trace_from_doc)
    trace_manager.insert(trace_from_mail)

    results = trace_manager.all()
    assert len(results) == 2
    labels = {r.label.value for r in results}
    assert labels == {"trace-doc", "trace-mail"}


@pytest.mark.integration
@pytest.mark.order(28)
def test_filter_relation_by_multi_role_player(db_with_multi_role_schema):
    """Test filtering relations by multi-role player entity."""
    doc = Document(document_id=DocumentId("doc-003"))
    mail = Email(subject=EmailSubject("Urgent"))

    Document.manager(db_with_multi_role_schema).insert(doc)
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace_from_doc = Trace(origin=doc, label=TraceLabel("doc-trace"))
    trace_from_mail = Trace(origin=mail, label=TraceLabel("mail-trace"))

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(trace_from_doc)
    trace_manager.insert(trace_from_mail)

    # Filter by document as role player
    doc_results = trace_manager.get(origin=doc)
    assert len(doc_results) == 1
    assert doc_results[0].label.value == "doc-trace"

    # Filter by email as role player
    mail_results = trace_manager.get(origin=mail)
    assert len(mail_results) == 1
    assert mail_results[0].label.value == "mail-trace"


@pytest.mark.integration
@pytest.mark.order(29)
def test_delete_relation_with_multi_role_filter(db_with_multi_role_schema):
    """Test deleting relations filtered by multi-role player."""
    doc = Document(document_id=DocumentId("doc-004"))
    mail = Email(subject=EmailSubject("Delete Test"))

    Document.manager(db_with_multi_role_schema).insert(doc)
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace_from_doc = Trace(origin=doc, label=TraceLabel("to-delete"))
    trace_from_mail = Trace(origin=mail, label=TraceLabel("to-keep"))

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(trace_from_doc)
    trace_manager.insert(trace_from_mail)

    # Delete only the document-origin trace
    trace_manager.filter(origin=doc).delete()

    remaining = trace_manager.all()
    assert len(remaining) == 1
    assert remaining[0].label.value == "to-keep"


@pytest.mark.integration
@pytest.mark.order(30)
def test_update_relation_with_multi_role(db_with_multi_role_schema):
    """Test updating a relation that uses a multi-role."""
    doc = Document(document_id=DocumentId("doc-005"))
    Document.manager(db_with_multi_role_schema).insert(doc)

    trace = Trace(origin=doc, label=TraceLabel("original-label"))
    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(trace)

    # Fetch, modify, and update
    fetched = trace_manager.get(origin=doc)[0]
    fetched.label = TraceLabel("updated-label")
    trace_manager.update(fetched)

    # Verify update
    updated = trace_manager.all()
    assert len(updated) == 1
    assert updated[0].label.value == "updated-label"


@pytest.mark.integration
@pytest.mark.order(31)
def test_update_with_chainable_multi_role(db_with_multi_role_schema):
    """Test chainable update_with on relations with multi-role."""
    doc = Document(document_id=DocumentId("doc-006"))
    mail = Email(subject=EmailSubject("Update With Test"))

    Document.manager(db_with_multi_role_schema).insert(doc)
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace_doc = Trace(origin=doc, label=TraceLabel("doc-v1"))
    trace_mail = Trace(origin=mail, label=TraceLabel("mail-v1"))

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(trace_doc)
    trace_manager.insert(trace_mail)

    # Update only document traces using chainable API
    def add_prefix(trace):
        trace.label = TraceLabel("prefix-" + trace.label.value)

    updated = trace_manager.filter(origin=doc).update_with(add_prefix)
    assert len(updated) == 1

    # Verify updates
    all_traces = trace_manager.all()
    labels = {t.label.value for t in all_traces}
    assert labels == {"prefix-doc-v1", "mail-v1"}


@pytest.mark.integration
@pytest.mark.order(32)
def test_multiple_relations_same_role_player(db_with_multi_role_schema):
    """Test creating multiple relations with the same entity as role player."""
    doc = Document(document_id=DocumentId("doc-007"))
    Document.manager(db_with_multi_role_schema).insert(doc)

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(Trace(origin=doc, label=TraceLabel("trace-1")))
    trace_manager.insert(Trace(origin=doc, label=TraceLabel("trace-2")))
    trace_manager.insert(Trace(origin=doc, label=TraceLabel("trace-3")))

    # All traces should have the same origin
    results = trace_manager.get(origin=doc)
    assert len(results) == 3
    labels = {r.label.value for r in results}
    assert labels == {"trace-1", "trace-2", "trace-3"}


@pytest.mark.integration
@pytest.mark.order(33)
def test_multi_role_with_three_entity_types(clean_db):
    """Test multi-role with three entity types."""

    class ReportId(String):
        pass

    class Report(Entity):
        flags = TypeFlags(name="report")
        report_id: ReportId = Flag(Key)

    class AuditLabel(String):
        pass

    class Audit(Relation):
        """Relation where origin can be Document, Email, or Report."""

        flags = TypeFlags(name="audit")
        origin: Role[Document | Email | Report] = Role.multi("origin", Document, Email, Report)
        audit_label: AuditLabel

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Document, Email, Report, Audit)
    schema_manager.sync_schema(force=True)

    # Create entities
    doc = Document(document_id=DocumentId("doc-audit"))
    mail = Email(subject=EmailSubject("Audit Email"))
    report = Report(report_id=ReportId("report-001"))

    Document.manager(clean_db).insert(doc)
    Email.manager(clean_db).insert(mail)
    Report.manager(clean_db).insert(report)

    # Create audits for each type
    audit_manager = Audit.manager(clean_db)
    audit_manager.insert(Audit(origin=doc, audit_label=AuditLabel("doc-audit")))
    audit_manager.insert(Audit(origin=mail, audit_label=AuditLabel("mail-audit")))
    audit_manager.insert(Audit(origin=report, audit_label=AuditLabel("report-audit")))

    results = audit_manager.all()
    assert len(results) == 3
    labels = {r.audit_label.value for r in results}
    assert labels == {"doc-audit", "mail-audit", "report-audit"}


@pytest.mark.integration
@pytest.mark.order(34)
def test_filter_by_attribute_with_multi_role(db_with_multi_role_schema):
    """Test filtering by attribute when relation has multi-role."""
    doc = Document(document_id=DocumentId("doc-008"))
    mail = Email(subject=EmailSubject("Attr Filter"))

    Document.manager(db_with_multi_role_schema).insert(doc)
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(Trace(origin=doc, label=TraceLabel("target")))
    trace_manager.insert(Trace(origin=mail, label=TraceLabel("other")))

    # Filter by attribute label
    results = trace_manager.get(label="target")
    assert len(results) == 1
    assert results[0].label.value == "target"


@pytest.mark.integration
@pytest.mark.order(35)
def test_expression_filter_with_multi_role(db_with_multi_role_schema):
    """Test expression-based filters on relations with multi-role."""
    doc1 = Document(document_id=DocumentId("doc-expr-1"))
    doc2 = Document(document_id=DocumentId("doc-expr-2"))
    mail = Email(subject=EmailSubject("Expr Test"))

    Document.manager(db_with_multi_role_schema).insert(doc1)
    Document.manager(db_with_multi_role_schema).insert(doc2)
    Email.manager(db_with_multi_role_schema).insert(mail)

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(Trace(origin=doc1, label=TraceLabel("alpha")))
    trace_manager.insert(Trace(origin=doc2, label=TraceLabel("beta")))
    trace_manager.insert(Trace(origin=mail, label=TraceLabel("gamma")))

    # Filter using expression on label
    results = trace_manager.filter(TraceLabel.eq(TraceLabel("beta"))).execute()
    assert len(results) == 1
    assert results[0].label.value == "beta"


@pytest.mark.integration
@pytest.mark.order(36)
def test_update_preserves_multi_role_player(db_with_multi_role_schema):
    """Test that updating a relation preserves its multi-role player."""
    doc = Document(document_id=DocumentId("doc-preserve"))
    Document.manager(db_with_multi_role_schema).insert(doc)

    trace = Trace(origin=doc, label=TraceLabel("before"))
    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert(trace)

    # Update the label
    fetched = trace_manager.get(origin=doc)[0]
    fetched.label = TraceLabel("after")
    trace_manager.update(fetched)

    # Verify role player is preserved
    updated = trace_manager.get(origin=doc)
    assert len(updated) == 1
    assert updated[0].label.value == "after"
    # Verify origin is still the same Document
    origin = updated[0].origin
    assert isinstance(origin, Document)
    assert origin.document_id.value == "doc-preserve"


@pytest.mark.integration
@pytest.mark.order(37)
def test_insert_many_with_multi_role(db_with_multi_role_schema):
    """Test insert_many with different entity types in multi-role."""
    doc = Document(document_id=DocumentId("doc-many"))
    mail = Email(subject=EmailSubject("Many Test"))

    Document.manager(db_with_multi_role_schema).insert(doc)
    Email.manager(db_with_multi_role_schema).insert(mail)

    traces = [
        Trace(origin=doc, label=TraceLabel("batch-doc-1")),
        Trace(origin=mail, label=TraceLabel("batch-mail-1")),
        Trace(origin=doc, label=TraceLabel("batch-doc-2")),
    ]

    trace_manager = Trace.manager(db_with_multi_role_schema)
    trace_manager.insert_many(traces)

    results = trace_manager.all()
    assert len(results) == 3

    doc_traces = trace_manager.get(origin=doc)
    assert len(doc_traces) == 2

    mail_traces = trace_manager.get(origin=mail)
    assert len(mail_traces) == 1
