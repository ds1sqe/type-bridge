# Model plan twins

Model manager calls run through the existing direct manager journey. The
private conformance build records their actual native plan lowerings behind
the existing plan trace hook. Ordinary builds compile out the sink. It adds
no public option and emits no caller values to ordinary diagnostic logs.
Selected fixture calls write a label file; the sink retains each lowered
plan under that label in order, and blank labels select no calls.

The four SDKs run the same 13 manager-filter recipes: six comparisons,
conjunction, all/count/exists/first, and the sibling query/filter reads.
Their Counter lifecycle inserts values `[1, 2]`, records the returned IIDs
as `counter-left` and `counter-right`, reads and counts them, updates the
left value to `11`, updates both, and deletes both. A staged typed count
also proves that a pending result becomes available after transaction
commit, even after its query session closes.

The shared Person and NetworkLink lifecycle captures single insert, put,
update, read, count and delete, with empty ordered fields and roles. Batch
put, insert, update and delete use the same named input records in every
SDK. Returning-delete manager calls capture entity and relation single and
batch snapshots in Python, Node and Rust. C manager returning-delete exports
are part of the ABI 2.0 migration; the coverage entries explicitly name the
three exposed SDKs until that migration adds the fourth twin.

Twins retain every selected native plan, including given rows. Each selected
call must execute exactly one plan. Returning-delete plans lower to a
snapshot statement followed by deletion in the same transaction. The
comparison and allowed binding renaming/IID substitution follow the query
twins' rules in [queries/plans](../../queries/plans/README.md).

`coverage.json` annotates every unchanged journey observation. Its 41
selected native call names must match the actual witnesses for their
specified SDKs exactly. Existing additional CRUD, validation, lifecycle and
resource-policy probes continue to run, and the frozen oracle answer comparison
remains required.
