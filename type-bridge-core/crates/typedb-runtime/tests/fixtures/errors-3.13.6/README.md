# TypeDB 3.13.6 failure traces

Captured live on 2026-10-15 using typedb-driver 3.13.6. Quoted schema names,
database names, values and instance IDs are replaced with placeholders. The
root code is supplied structurally by Error::code(), independently of prose.

- stc2: two writes update the same checked-cardinality ownership, one commit wins.
- cnt5: one owner receives two values under @card(0..1), rejected at commit.
- cnt5_plays: one player receives two participations under plays @card(0..1),
  rejected at commit (DVL12 cause).
- cnt9: a duplicate @key value is rejected during query execution.
- cnt6_regex and cnt7_range: invalid attribute values are rejected during query.
- require_deleted_key: delete the last owner of an attribute before another
  transaction commits an attachment of that attribute to another owner.
- deleting_required_key: commit the attachment before committing that deletion.
- paused: pause the owned server after query dispatch, then attempt commit;
  retain the driver's actual failure rather than synthesizing a timeout.

The deterministic fixture tests classify each trace without preserving prose.
The runtime live probe also confirms the outcomes through its admitted connection.
