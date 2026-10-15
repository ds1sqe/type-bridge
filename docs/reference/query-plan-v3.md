# Query-plan V3 wire format

V3 is an ordered, schema-checked read/write program encoded as canonical UTF-8
JSON. A producer can construct these bytes without Rust. SDKs delegate encoding
and validation to Rust. This document specifies the format and intended TypeQL
lowering; execution and lowering are separate implementation phases.

## Root object

Required members are `format`, `bindings`, `inputs`, `functions`, `pipeline`,
`output`, `operation`, `managed_semantics`, and `required_capabilities`.
`format` is exactly `typebridge.query-plan/v3`. Optional `given_rows` and
`compatibility` members are omitted when absent. Empty required arrays remain
present. Unknown fields, duplicate keys, unknown tags and forged capability
claims are rejected. The schema is provided separately to the decoder; the
embedded managed fingerprint is an expected identity, not schema authority.
Execution must compare it with the receiving authority before sending a query.

| Member | Shape |
| --- | --- |
| `bindings` | Ordered `[{"id":0,"variable":"item"},...]` |
| `inputs` | Ordered `[{"id":0,"optional":false,"public_name":"item_id","value_type":"string"},...]` |
| `functions` | Ordered local function declarations or `[]` |
| `pipeline` | Nonempty ordered stage array |
| `output` | Rows or documents, below |
| `operation` | Terminal operation, below |
| `managed_semantics` | Managed schema fingerprint object |
| `required_capabilities` | Sorted duplicate-free array of derived strings |

Binding IDs and input IDs are separate dense zero-based unsigned sequences.
Bindings number at most 256. Variables and public names are unique identifiers matching
`[a-z][a-z0-9_]*`, at most 128 UTF-8 bytes. Array order and variable spelling
are part of identity.

A type is `{"kind":"entity","label":"account"}`, with kind `entity`,
`relation` or `attribute`. An attribute is a label string. A role is
`{"declaring_relation":"membership","label":"member"}`. An ownership is
`{"attribute":"score","owner":{"kind":"entity","label":"account"}}`.
Labels are at most 255 UTF-8 bytes: first character `_` or Unicode XID_Start,
then Unicode XID_Continue or `-`. The exact reserved labels are `with`, `given`,
`match`, `fetch`, `update`, `define`, `undefine`, `redefine`, `insert`, `put`,
`delete`, `end`, `entity`, `relation`, `attribute`, `role`, `asc`, `desc`, `struct`,
`fun`, `return`, `alias`, `sub`, `owns`, `as`, `plays`, `relates`, `iid`, `isa`,
`links`, `has`, `is`, `or`, `not`, `try`, `in`, `true`, `false`, `of`, `from`,
`first`, `last`. Function-call names additionally exclude builtin spellings
`abs`, `ceil`, `floor`, `iid`, `label`, `len`, `max`, `min`, `round`.
The common serialized semantic identity ceiling is 512 bytes.

Fingerprints contain `algorithm` (`sha256`), `canonicalization`, `digest` (64
lowercase hex digits), `domain`, and an optional `semantic_profile`. The managed
fingerprint uses domain `typebridge.schema.managed-semantic`, canonicalization
`typebridge.managed-semantic/v1`, and the provider profile. Its digest is opaque
and cannot be derived from plan bytes.

## Scalars, operands and expressions

| Domain | Wire value |
| --- | --- |
| string | `{"kind":"string","value":"text"}` |
| long | `{"kind":"long","value":"-42"}`; signed 64-bit decimal string |
| double | `{"bits":"3ff0000000000000","kind":"double"}`; finite binary64 bits, 16 lowercase hex digits |
| boolean | `{"kind":"boolean","value":true}` |
| decimal | `{"kind":"decimal","value":"1.25"}` |
| date | `{"kind":"date","value":"2026-10-15"}` |
| datetime | `{"kind":"datetime","value":"2026-10-15T12:00:00"}` |
| datetime_tz | `{"kind":"datetime_tz","value":{"effective_offset_seconds":0,"local":"2026-10-15T12:00:00","zone":{"kind":"utc"}}}` |
| duration | `{"kind":"duration","value":"P1DT7200S"}` |

Longs have no leading plus or redundant zeros. Decimal strings have no exponent,
`dec` suffix or redundant leading/trailing zeros; zero is `0`. At most 19
fractional digits are accepted. The whole part fits signed 64-bit; the negative
minimum cannot have a nonzero fractional part. Double bits
preserve negative zero; NaN and infinity fail. Dates are Gregorian in years -262143 through 262142, with four year digits for
0..9999, a leading plus for larger positive years and a minus for negative
years. Month/day are two digits. Datetimes append `T` and `HH:MM:SS`, optionally
1..9 fractional second digits with no trailing zero; hours are 0..23 and minutes
and seconds 0..59. There are no leap seconds.

Datetime-tz's value is an object with `local`, `zone` and
`effective_offset_seconds`. Zone is `{"kind":"utc"}` (effective offset zero),
`{"kind":"offset_seconds","seconds":3600}` (same nonzero effective offset),
or `{"kind":"named","name":"Europe/London"}` with provider-resolved effective
offset. Absolute offsets are at most 86399 seconds. A named zone is 1..255 ASCII
bytes from letters, digits, `/`, `_`, `-`, `+`; the producer supplies the
resolved offset without relying on receiver timezone-database versions.

Duration spelling is optional `-`, `P`, optional unsigned months plus `M`,
optional unsigned days plus `D`, then optional `T` seconds plus `S`. Seconds may
have 1..9 fractional digits with no trailing zero. Components are u64; omit zero
components and use `PT0S` for zero, never negative zero. There are no year/week/
hour/minute components in canonical bytes; convert those to months/days/seconds.

Operands have exactly one of these shapes:

```json
{"binding":0,"kind":"binding"}
{"kind":"literal","value":{"kind":"long","value":"1"}}
{"column":0,"kind":"input"}
```

Expressions use adjacent `kind` / `value` tags:

| Kind | `value` |
| --- | --- |
| `operand` | One operand |
| `arith` | `{"op":"sub","lhs":expression,"rhs":expression}` |
| `call` | `{"func":"abs","args":[expression,...]}` |

Arithmetic operators are `add`, `sub`, `mul`, `div`, `mod`, `pow`. Both arguments
must be numeric. Add/sub/mul promote double over decimal over long. Div/pow
always return double. Mod returns long for two longs and double otherwise.
These rules match TypeDB 3.13.6.

| Builtin | Arguments | Result |
| --- | --- | --- |
| abs | One numeric | Same domain |
| ceil, floor, round | One decimal or double | Long |
| min, max | Exactly two numerics of the same domain | Same domain |
| length | One string | Long |

Binding operands must be present scalar bindings with schema-inferred attribute
or function-return domains. Input operands must reference required columns.
Write assignments require the exact target attribute domain: arithmetic
promotion does not implicitly cast its result back to that domain.

## Match patterns

A match is `{"kind":"match","patterns":[...]}`. Patterns form a conjunction.
In the table, B is a binding ID, T a type, R a role, O an operand, P a pattern.
Member names are literal wire names, and each listed member is required.

| `kind` | Additional members |
| --- | --- |
| isa | `binding:B`, `type_id:T`, `include_subtypes:boolean` |
| has | `owner:B`, `attribute:B`, `attribute_id:string` |
| links | `relation:B`, `relation_id:T`, `players:[{"player":B,"role":R},...]` |
| value | `left:O`, `comparator:string`, `right:O` |
| or | `branches:[[P,...],...]` |
| not, try | `patterns:[P,...]` |
| reachable | `source:B`, `target:B`, `relation:T`, `role_from:R`, `role_to:R`, `min_depth:integer`, `max_depth:integer` |
| function_call | `assigned:B`, `function:string`, `arguments:[O,...]` |
| let | `binding:B`, `expr:expression` |

Comparators are `equal`, `not_equal`, `less`, `less_or_equal`, `greater`,
`greater_or_equal`. Boolean branches are nonempty. Reachability bounds satisfy
`0 <= min_depth <= max_depth <= 64` (the receiver's predicate-depth limit),
with root-only reachability and expanded-hop node-budget checks. Functions use checked declarations and
argument/return types. Nested boolean bodies and local functions retain the
released read-pattern vocabulary: let is a V3 root-match pattern.

Let assigns a fresh declared scalar binding in source order. It cannot overwrite
an earlier let, concept, attribute or function result, including one produced
in only one disjunction arm. Forward/cyclic expression references fail. Positive
matches establish mandatory bindings; disjunction exports the intersection of
mandatory bindings. Try-only bindings are optional; negated bindings do not
escape. Every write target must be mandatory and alive.

Local functions have `name`, `bindings`, `parameters`, `body`, `returns`.
Parameters are ordered label strings. Bindings use an independent dense scope.
Returns is `{"input":0,"reducer":"count","value_type":"long"}`. Local returns
admit only count (long) and sum (numeric), total on empty streams. Bodies are
flat isa/has/links/value patterns; parameters declare leading bindings by schema
type label. Undeclared or recursive calls are invalid. Pipeline reducers also
admit `max`, `min`, `mean`, `median`, `std` under the read contract's domain and
empty-stream checks.

## Stages and output

The first stage is match, insert or put. Match can occur only first. Writes may
follow in any order before the read tail. Write stage payloads are nonempty.

| `kind` | Additional members |
| --- | --- |
| insert, put | `things:[Thing,...]` |
| update | `sets:[Set,...]` |
| delete | `items:[Delete,...]` |

A Thing has `binding`, `type_id`, `fields`, `players`. Fields are ordered pairs
`[[ownership,operand],...]`; players are ordered pairs `[[role,binding],...]`.
Absent fields/players use empty arrays. Insert/put bind fresh variables of
concrete declared entity/relation types; attribute types are refused. Relations
require at least one role player, and a player may be created in the same stage. Ownership, relates and plays facts
must be declared, including inherited declarations.

A Set is `{"kind":"field","owner":B,"field":ownership,"operand":O}` or
`{"kind":"link","relation":B,"role":R,"player":B}`. Key ownerships cannot
be updated, including keys declared on any descendant of a subtype-inclusive
match. Descendants are enumerated from the declared subtype facts. A Delete is `{"kind":"thing","binding":B}`,
`{"kind":"has","owner":B,"field":ownership}`, or
`{"kind":"links","relation":B,"role":R,"player":B}`. Thing deletion ends
its binding lifetime after that stage; scalar let/function results are not
thing deletion targets; later stages/output cannot refer to it.
Ownership/link deletion leaves the owner/relation alive. All inferred possible
owner/player types must satisfy the requested fact; inference is conservative.

The read tail is an ordered subsequence of this list, each stage at most once:

| `kind` | Additional members |
| --- | --- |
| select, require | `bindings:[B,...]` |
| distinct | None |
| reduce | `assignments:[{"assigned":B,"input":B,"reducer":"count"},...]`, `groups:[B,...]` |
| sort | `terms:[{"binding":B,"direction":"ascending"},...]` |
| offset, limit | `rows:unsigned_integer` |

Sort directions are `ascending`/`descending`. Select restricts scope; require
makes visible optional bindings mandatory. Reduce exposes grouping bindings
and fresh reduction results. Output and ordering must respect the resulting
scope. Duplicate selected/output bindings and sort terms fail. Offset and limit require
an explicit preceding sort.

Rows output is `{"kind":"rows","columns":[B,...]}`. Documents output is
`{"kind":"documents","fields":[{"key":"item","source":{"kind":"binding","binding":0}}]}`.
A source may instead be
`{"kind":"attribute_list","owner":0,"attribute":"tag"}`. Document keys
are unique validated output names.

Operations are `{"kind":"rows"}`, `{"kind":"count"}`, `{"kind":"exists"}`,
`{"kind":"write","returns":"matched"}`, or
`{"kind":"write","returns":"rows"}`. Write requires a write stage; read
operations reject write stages. Rows/write-rows require a projection;
write-matched requires `{"kind":"rows","columns":[]}`. Count/exists allow
empty rows output. Write-matched counts matched pipeline rows, not individual
field changes. Write-rows projects the same pipeline after the writes.

## Given rows

Optional `given_rows` is a nonempty array of positional rows. Each row contains
exactly one canonical scalar or null per input column. Domains must match;
null is permitted only for optional columns. Supplied rows require nonempty
input declarations. The ceiling is 4,096 rows and 4 MiB canonical row-array
bytes. Bound rows participate in the plan fingerprint.

Omission preserves a reusable template. A template with inputs must be bound
before execution. User values, including literals, become typed given
parameters; they are never interpolated into TypeQL text.

## Canonicalization and fingerprint

Canonical JSON is compact UTF-8, without BOM, whitespace or terminal newline.
Object keys are lexicographically sorted by Unicode scalar value. Array order
is retained. Escape quote, backslash and controls using JSON's short control
escapes where available, otherwise lowercase `\u00xx`. Do not escape `/` or
printable Unicode, or normalize Unicode. IDs/counts/bounds are minimal unsigned
JSON integers; scalar longs are strings and doubles are bits. Required members
are not omitted just because their value is empty or false.

Decode, validate, re-encode and require exact byte equality. Whitespace,
noncanonical key order, alternate scalar spelling and incorrect capability
lists therefore fail.

| Ceiling | Value |
| --- | --- |
| Complete canonical artifact | 16 MiB |
| JSON depth (root = 1) | 64 |
| Members in one collection | 65,536 |
| One string/key | 1 MiB |
| Bindings / output slots | 256 / 16 |
| Predicate/expression nodes / depth | 4,096 / 64 |
| Boolean children / order terms | 256 / 64 |
| Cross-join pairs | 1,024 |
| Given rows / canonical row bytes | 4,096 / 4 MiB |

JSON depth may reject a tree before its semantic depth ceiling. Receivers can
choose tighter supported limits; limits are local policy and not hashed.
Diagnostics contain stable bounded metadata without user values or fragments.

The plan fingerprint uses domain `typebridge.query.plan`, canonicalization
`typebridge.query-plan-c14n/v3`, SHA-256 and no outer semantic profile. Define
`frame(x)` as an unsigned 64-bit big-endian byte length followed by x. Hash:

```text
UTF8("typebridge.fingerprint/v1") || 0x00
|| frame(UTF8("typebridge.query.plan"))
|| frame(UTF8("typebridge.query-plan-c14n/v3"))
|| 0x00
|| frame(canonical_plan_bytes)
```

Render the digest as 64 lowercase hex digits. The managed fingerprint already
carries its profile inside the hashed bytes. Operation and bound row changes
produce a different plan digest.

## Capabilities

Recompute the sorted union from syntax. V3 includes `query.plan.v3`; released
read syntax also derives `query.plan`. New stages add `query.write.insert`,
`query.write.put`, `query.write.update`, `query.write.delete`. Let adds
`query.let`; expression operators/builtins add `query.expr.` plus their exact
wire name, including `query.expr.length`.

The retained read vocabulary is `query.pattern.isa`, `query.pattern.isa-subtypes`,
`query.pattern.has`, `query.pattern.links`, `query.pattern.value`,
`query.pattern.negation`, `query.pattern.disjunction`, `query.pattern.try`,
`query.pattern.function-call`, `query.pattern.reachable`, `query.function.local`,
`query.input.columns`, `query.stage.select`, `query.stage.require`,
`query.stage.distinct`, `query.stage.reduce`, `query.stage.sort`,
`query.stage.offset`, `query.stage.limit`, `query.output.rows`, and
`query.output.documents`. Each construct contributes its corresponding feature.
Inputs or any literal/input operand add `query.input.given-rows`. Lifted
compatibility syntax additionally retains its V2 capability derivation.
Providers must advertise every required feature.

## TypeQL lowering

Lower stages in order into one pipeline in one transaction. Generate hygienic
variables, resolve labels through checked identities and bind values. These
are illustrative forms, not a snapshot read followed by a separate mutation.

| Construct | TypeQL form |
| --- | --- |
| isa / has / links | `$x isa account;` / `$x has score $s;` / `$r links (member: $x);` |
| value | `$s >= $minimum;` and corresponding comparator symbols |
| or / not / try | `{ ... } or { ... };` / `not { ... };` / `try { ... };` |
| let | `let $next = $stock - $one;` |
| arithmetic | Parenthesized `+`, `-`, `*`, `/`, `%`, `^` expressions |
| builtins | Same names except length becomes `len($text)` |
| insert / put | `insert $o isa order, has order-id $id;` / corresponding put stage |
| field / role update | `update $x has score == $next;` / `update $r links (member: $new);` |
| delete thing / ownership / link | `delete $x;` / `delete has $old_score of $x;` / `delete links (member: $old) of $r;` |
| select / require / distinct | `select $x;` / `require $x;` / `distinct;` |
| reduce | `reduce $n = count($x) groupby $group;` |
| sort / offset / limit | `sort $x asc;` / `offset 10;` / `limit 10;` |
| output / operation | Resolve the declared rows, documents, count, presence or write result from that pipeline |

Ownership deletion first binds the owned attribute (for example,
`$x has score $old_score`) in that same pipeline, then deletes that ownership
using the attribute variable. It does not render a field label as a deletable.

Bounded reachability uses a checked local function over declared roles; function
calls use checked let assignments and definitions. Given rows seed that same
pipeline. A touch assigns the matched value back to its field, retaining a
write conflict edge. Relation reassignment deletes the old role link before
assigning the new player. Write-row projection follows mutation in that same
pipeline. These lowering rules are documentation here; V3 lowering code belongs
to the subsequent execution phase.

## Versioning

V1/V2 bytes and fingerprints are frozen. `QueryPlan::to_v3(&schema)` revalidates
against declared schema with the original plan's limits and returns a result.
It preserves bindings,
inputs, ordered read stages, output, local functions, managed semantics and V2
compatibility. It chooses rows operation and omits given rows. Bind invocation
rows and choose count/exists explicitly as needed. The new discriminator,
capabilities and c14n version produce a V3 fingerprint, not the old digest.

Native V3 producers normally omit compatibility. Lifted plans preserve the
released object containing `allowed_cross_joins`, nullable `model_query` and
nullable `predicate` under the [V2 contract](../development/typed-query-contract.md).
V3 does not reinterpret those model-query predicates or terminal shapes.

A changed tag, required field, validation meaning, canonical encoding, numeric
promotion, capability derivation or stage semantics requires a new format and
canonicalization version. New closed constructs require V4; a V3 receiver
rejects unknown syntax instead of silently losing meaning.

## Worked canonical examples

The following are exact files from `tests/contracts/plan_v3`, without the
Markdown line terminator. A Rust test validates them under the separate
`schema.json`, compares fixture bytes and recomputes the displayed fingerprints.
Their managed fingerprint identifies fixture authority, not a production schema.

### purchase

Match one item and buyer, check stock and balance, compute decrements, update both and insert the purchase relation with buyer and item players, then return it.

<!-- plan-v3-example: purchase.json -->

```json
{"bindings":[{"id":0,"variable":"item"},{"id":1,"variable":"buyer"},{"id":2,"variable":"stock"},{"id":3,"variable":"balance"},{"id":4,"variable":"price"},{"id":5,"variable":"next_stock"},{"id":6,"variable":"next_balance"},{"id":7,"variable":"purchase"},{"id":8,"variable":"item_key"},{"id":9,"variable":"buyer_key"}],"format":"typebridge.query-plan/v3","functions":[],"given_rows":[[{"kind":"string","value":"purchase-1"},{"kind":"string","value":"item-1"},{"kind":"string","value":"buyer-1"}]],"inputs":[{"id":0,"optional":false,"public_name":"new_purchase_id","value_type":"string"},{"id":1,"optional":false,"public_name":"expected_item_id","value_type":"string"},{"id":2,"optional":false,"public_name":"expected_buyer_id","value_type":"string"}],"managed_semantics":{"algorithm":"sha256","canonicalization":"typebridge.managed-semantic/v1","digest":"4cafd679447c7f0dd1a99a702ebb9e7bf2b70c2eb3f44ab5ad5fac6e7b858a07","domain":"typebridge.schema.managed-semantic","semantic_profile":"typedb-3.13.6/v1"},"operation":{"kind":"write","returns":"rows"},"output":{"columns":[7],"kind":"rows"},"pipeline":[{"kind":"match","patterns":[{"binding":0,"include_subtypes":false,"kind":"isa","type_id":{"kind":"entity","label":"item"}},{"binding":1,"include_subtypes":false,"kind":"isa","type_id":{"kind":"entity","label":"account"}},{"attribute":2,"attribute_id":"stock","kind":"has","owner":0},{"attribute":3,"attribute_id":"balance","kind":"has","owner":1},{"attribute":4,"attribute_id":"price","kind":"has","owner":0},{"attribute":8,"attribute_id":"item-id","kind":"has","owner":0},{"attribute":9,"attribute_id":"id","kind":"has","owner":1},{"comparator":"equal","kind":"value","left":{"binding":8,"kind":"binding"},"right":{"column":1,"kind":"input"}},{"comparator":"equal","kind":"value","left":{"binding":9,"kind":"binding"},"right":{"column":2,"kind":"input"}},{"comparator":"greater","kind":"value","left":{"binding":2,"kind":"binding"},"right":{"kind":"literal","value":{"kind":"long","value":"0"}}},{"comparator":"greater_or_equal","kind":"value","left":{"binding":3,"kind":"binding"},"right":{"binding":4,"kind":"binding"}},{"binding":5,"expr":{"kind":"arith","value":{"lhs":{"kind":"operand","value":{"binding":2,"kind":"binding"}},"op":"sub","rhs":{"kind":"operand","value":{"kind":"literal","value":{"kind":"long","value":"1"}}}}},"kind":"let"},{"binding":6,"expr":{"kind":"arith","value":{"lhs":{"kind":"operand","value":{"binding":3,"kind":"binding"}},"op":"sub","rhs":{"kind":"operand","value":{"binding":4,"kind":"binding"}}}},"kind":"let"}]},{"kind":"update","sets":[{"field":{"attribute":"stock","owner":{"kind":"entity","label":"item"}},"kind":"field","operand":{"binding":5,"kind":"binding"},"owner":0},{"field":{"attribute":"balance","owner":{"kind":"entity","label":"account"}},"kind":"field","operand":{"binding":6,"kind":"binding"},"owner":1}]},{"kind":"insert","things":[{"binding":7,"fields":[[{"attribute":"purchase-id","owner":{"kind":"relation","label":"purchase"}},{"column":0,"kind":"input"}]],"players":[[{"declaring_relation":"purchase","label":"buyer"},1],[{"declaring_relation":"purchase","label":"item"},0]],"type_id":{"kind":"relation","label":"purchase"}}]}],"required_capabilities":["query.expr.sub","query.input.columns","query.input.given-rows","query.let","query.output.rows","query.pattern.has","query.pattern.isa","query.pattern.value","query.plan","query.plan.v3","query.write.insert","query.write.update"]}
```

Fingerprint: `ceeed10049398f899726749f1f28beb547d93201d04d3f3799790c49cdb81c09`.

### touch

Match a doctor and its on-call value, then assign that value back and return the doctor.

<!-- plan-v3-example: touch.json -->

```json
{"bindings":[{"id":0,"variable":"account"},{"id":1,"variable":"peer"},{"id":2,"variable":"total"}],"format":"typebridge.query-plan/v3","functions":[],"inputs":[],"managed_semantics":{"algorithm":"sha256","canonicalization":"typebridge.managed-semantic/v1","digest":"4cafd679447c7f0dd1a99a702ebb9e7bf2b70c2eb3f44ab5ad5fac6e7b858a07","domain":"typebridge.schema.managed-semantic","semantic_profile":"typedb-3.13.6/v1"},"operation":{"kind":"write","returns":"rows"},"output":{"columns":[0],"kind":"rows"},"pipeline":[{"kind":"match","patterns":[{"binding":0,"include_subtypes":false,"kind":"isa","type_id":{"kind":"entity","label":"doctor"}},{"attribute":1,"attribute_id":"on-call","kind":"has","owner":0}]},{"kind":"update","sets":[{"field":{"attribute":"on-call","owner":{"kind":"entity","label":"doctor"}},"kind":"field","operand":{"binding":1,"kind":"binding"},"owner":0}]}],"required_capabilities":["query.output.rows","query.pattern.has","query.pattern.isa","query.plan","query.plan.v3","query.write.update"]}
```

Fingerprint: `820cec8799b567f4a182d52014290ae2b00edab1d671be36592c7ea71d08166e`.

### relation_assign

Remove the old member link and assign the new account in the same pipeline.

<!-- plan-v3-example: relation_assign.json -->

```json
{"bindings":[{"id":0,"variable":"account"},{"id":1,"variable":"peer"},{"id":2,"variable":"total"}],"format":"typebridge.query-plan/v3","functions":[],"inputs":[],"managed_semantics":{"algorithm":"sha256","canonicalization":"typebridge.managed-semantic/v1","digest":"4cafd679447c7f0dd1a99a702ebb9e7bf2b70c2eb3f44ab5ad5fac6e7b858a07","domain":"typebridge.schema.managed-semantic","semantic_profile":"typedb-3.13.6/v1"},"operation":{"kind":"write","returns":"rows"},"output":{"columns":[1],"kind":"rows"},"pipeline":[{"kind":"match","patterns":[{"binding":0,"include_subtypes":false,"kind":"isa","type_id":{"kind":"entity","label":"account"}},{"binding":1,"include_subtypes":false,"kind":"isa","type_id":{"kind":"relation","label":"membership"}},{"binding":2,"include_subtypes":false,"kind":"isa","type_id":{"kind":"entity","label":"account"}}]},{"items":[{"kind":"links","player":0,"relation":1,"role":{"declaring_relation":"membership","label":"member"}}],"kind":"delete"},{"kind":"update","sets":[{"kind":"link","player":2,"relation":1,"role":{"declaring_relation":"membership","label":"member"}}]}],"required_capabilities":["query.output.rows","query.pattern.isa","query.plan","query.plan.v3","query.write.delete","query.write.update"]}
```

Fingerprint: `0d0f3e56c870c31aa254c8819235eef9a9bb96196c7d5a167cd000691bc20117`.

### batch

Two given rows insert accounts using typed ID and score columns, returning matched rows.

<!-- plan-v3-example: batch.json -->

```json
{"bindings":[{"id":0,"variable":"account"},{"id":1,"variable":"peer"},{"id":2,"variable":"total"}],"format":"typebridge.query-plan/v3","functions":[],"given_rows":[[{"kind":"string","value":"account-1"},{"kind":"long","value":"1"}],[{"kind":"string","value":"account-2"},{"kind":"long","value":"2"}]],"inputs":[{"id":0,"optional":false,"public_name":"new_id","value_type":"string"},{"id":1,"optional":false,"public_name":"new_score","value_type":"long"}],"managed_semantics":{"algorithm":"sha256","canonicalization":"typebridge.managed-semantic/v1","digest":"4cafd679447c7f0dd1a99a702ebb9e7bf2b70c2eb3f44ab5ad5fac6e7b858a07","domain":"typebridge.schema.managed-semantic","semantic_profile":"typedb-3.13.6/v1"},"operation":{"kind":"write","returns":"matched"},"output":{"columns":[],"kind":"rows"},"pipeline":[{"kind":"insert","things":[{"binding":0,"fields":[[{"attribute":"id","owner":{"kind":"entity","label":"account"}},{"column":0,"kind":"input"}],[{"attribute":"score","owner":{"kind":"entity","label":"account"}},{"column":1,"kind":"input"}]],"players":[],"type_id":{"kind":"entity","label":"account"}}]}],"required_capabilities":["query.input.columns","query.input.given-rows","query.output.rows","query.plan","query.plan.v3","query.write.insert"]}
```

Fingerprint: `c3a2e9bc496a9807ae6a32cb73c43f1f1601e066deec384dee1470369a89e5c0`.
