# Model plan twins

Model manager calls run through the existing direct manager journey. The
private conformance build records their actual native plan lowerings behind
the existing plan trace hook. Ordinary builds compile out the sink. It adds
no public option and emits no caller values to ordinary diagnostic logs.
Selected fixture calls write a label file; the sink retains each lowered
plan under that label in order, and blank labels select no calls.

The four SDKs run the same 13 manager-filter recipes: six comparisons,
conjunction, all/count/exists/first, and the sibling query/filter reads.
They also run one named Counter lifecycle: insert values `[1, 2]`, record
the returned IIDs as `counter-left` and `counter-right`, update the left
value to `11`, then delete both. These three write calls retain their full
ordered plan stacks, including given rows. Existing additional CRUD probes
continue to run alongside this shared section.
Twins retain every selected native plan. The comparison and allowed binding
renaming/IID substitution follow the query twins' rules in
[queries/plans](../../queries/plans/README.md). Native plans carry their own
`given_rows`; those same rows are retained in each stack entry.

Existing ordered-manager operations use the explicitly retained raw
remainder until milestone 06. The recorder never invents native plans for
that path. The normal Models journey and frozen V2 observation comparison
continue to validate those operations.

`coverage.json` names all unchanged journey observations and distinguishes
the 16 selected native call twins, host-only checks, retained raw operations,
and additional answer-gate probes. Its selected call inventory must match
the four actual witnesses exactly.
