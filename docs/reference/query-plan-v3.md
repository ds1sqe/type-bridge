# Query-plan V3 wire format

V3 is an ordered, schema-checked read/write program encoded as canonical UTF-8
JSON. A producer can construct these bytes without Rust. SDKs delegate encoding
and validation to Rust. This document specifies the format, its TypeQL
lowering, the shared builder and batch-split rules, and the staged remote
envelope that carries ordered stacks of plans.

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
| iid | `binding:B`, `iid:string` |

Comparators are `equal`, `not_equal`, `less`, `less_or_equal`, `greater`,
`greater_or_equal`. Boolean branches are nonempty. Reachability bounds satisfy
`0 <= min_depth <= max_depth <= 64` (the receiver's predicate-depth limit),
with root-only reachability and expanded-hop node-budget checks. Functions use checked declarations and
argument/return types. Nested boolean bodies and local functions retain the
released read-pattern vocabulary: let and iid are V3 root-match patterns.
An iid pattern names a canonical thing IID (`0x` followed by 1..256
hexadecimal digits) and constrains a mandatory binding that positive patterns
of the same match type only as entities or relations; it adds
`query.pattern.iid`.

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
| attach | `items:[Attach,...]` |
| fetch | `thing:B`, `roles:[{"role":R,"required":boolean},...]` |

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
`{"kind":"links","relation":B,"role":R,"player":B}`, or
`{"kind":"players","relation":B,"role":R}`. A players deletion removes every
player of one declared role, as an ownership deletion removes every value;
every possible type of the relation binding must descend from the role's
declaring relation. Followed by an attach, it replaces a multi-player role.
Thing deletion ends its binding lifetime after that stage (all thing
deletions of a stage end together at its boundary, not in item order);
scalar let/function results are not thing deletion targets; later
stages/output cannot refer to it.
Ownership/link/players deletion leaves the owner/relation alive. All inferred possible
owner/player types must satisfy the requested fact; inference is conservative.

An Attach is `{"kind":"has","owner":B,"field":ownership,"operand":O}` or
`{"kind":"links","relation":B,"role":R,"player":B}`. It adds a value or a
player to a bound thing and keeps the existing ones, which `update` (a
replacement, valid only up to one value) cannot do for multi-valued
ownerships and roles. Key ownerships cannot be attached. Targets, operands
and roles are checked as for update. An attach adds `query.write.attach`.

A fetch is `{"kind":"fetch","thing":B,"roles":[{"required":true,"role":R},...]}`
and is normally the last stage. It answers one hydration document per row for a
present (mandatory, not optional) binding typed only as entities or
relations, so the operation is `rows` or `write` returning `rows`, and the
declared output is `{"kind":"rows","columns":[]}`. Each listed role must be
declared, every possible type of the thing must descend from the role's
declaring relation, and a role is listed once. Document keys are fixed:
`_iid`, `_type` (the exact type label), `attributes` (every owned attribute),
and for the role at position `N`: `_role_N_iid`, `_role_N_type`,
`_role_N_attributes`. A required role must be filled (an unfilled one drops
the row); an optional role's keys are absent when it is unfilled; several
players of a role give one document per player combination. A fetch adds
`query.fetch.thing`.

An opt-in deletion snapshot permits a fetch immediately before a terminal
`delete`, with `write` returning `rows` and empty output columns. It requires
`query.write.delete-snapshot` in addition to the ordinary fetch/delete
capabilities. Its answer is validated against the environment before deletion;
the deleted binding remains unavailable afterward. The delete must remove
exactly the fetched thing. Write prefixes are refused because executing the
prefix for both capture and deletion would repeat mutations. Existing plain
delete plans, answers and fingerprints are unchanged.

`query.write.attach` and `query.fetch.thing` are advertised by
`GET /v3/capabilities` and admitted on the staged route. Fetch answers use
schema-validated document evidence. The fixtures `attach.json`,
`replace_players.json` (players deletion then attach), `fetch_entity.json`
and `fetch_relation.json` in `tests/contracts/plan_v3` show their canonical
bytes.

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

## Batch splitting

A batch is one plan over all of its rows as `given` and fires once. It is
split only when its rows would pass a limit, into same-shape plans that run
in the same transaction, so the batch stays all-or-nothing. Every language
binding and the proxy call the one pure function
`type_bridge_contract::split::split`, so they split identically.

The input is the encoded plan size and an ordered list of units. A unit is
the `given` rows one input row produces, with their encoded bytes; it is
never divided. `SplitLimits::CANONICAL` is 4,096 rows and the 32 MiB
envelope; `SplitLimits::new` lets a caller tighten them. Each chunk holds at most
`max_rows` rows and at most `max_bytes` minus the plan bytes of row bytes.

Chunks are contiguous, order-preserving ranges of unit indexes, filled
greedily: a batch within the limits is exactly one range, the result depends
only on its inputs, and an empty batch yields no ranges. For example, 10,000
one-row units give `0..4096`, `4096..8192`, `8192..10000`. The split is
refused with `PlanTooLarge` when the plan leaves no byte budget for rows, and
with `UnitTooLarge` (naming the first such unit) when one unit alone exceeds
the row or byte budget.

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
`query.write.put`, `query.write.update`, `query.write.delete`,
`query.write.attach` and `query.fetch.thing`. Let adds `query.let`; an iid
pattern adds `query.pattern.iid`; expression operators/builtins add
`query.expr.` plus their exact wire name, including `query.expr.length`.

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

Derivation and advertisement are separate. `query_plan_v3_capability_vocabulary()`
is the V2 vocabulary (which includes `query.pattern.iid`) plus
`query.plan.v3`, the insert/update/delete/put write capabilities, `query.let`
and every `query.expr.*` name. The staged advertisement at
`GET /v3/capabilities` is the V2 advertisement's capabilities, that
vocabulary, `query.remote.envelope-v3` and `query.remote.write`.
`query.write.attach` and `query.fetch.thing` are included in that vocabulary.

## Plan builder

`query_plan::v3::PlanBuilder` is the one rule set every SDK builder calls, so
the same program gives the same canonical bytes in every language:

- Binding IDs are dense and follow creation order; input column IDs follow
  `input` call order.
- `exact`, `subtypes`, `field`, `links`, `compare` and `let_` add to the
  plan's single leading `match`, in call order. A plan with no such call has
  no match stage.
- Write calls append stages in call order. Consecutive calls of the same kind
  share one stage: `update`/`update_player` one update, `insert` one insert,
  `put` one put, and `delete`/`delete_field`/`delete_player` one delete.
  `thing_field` and `thing_player` extend an inserted or put thing.
- `row` appends a given row; with no rows the plan is a template.

Derived variable names come from labels. `exact`, `subtypes`, `insert` and
`put` use the type label's stem: ASCII letters are lowercased, digits and `_`
are kept, every other character becomes `_`, and a stem not starting with a
lowercase letter gains a `v` prefix (`purchase-id` gives `purchase_id`).
`field` uses `{owner variable}_{attribute stem}` (`item_stock`). A stem is cut
to 120 bytes; if it is taken, `_2`, `_3`, ... is appended, using the first
unused suffix (a second `item` is `item_2`). Explicit names (`input`, `let_`)
are used as given and must be fresh; bindings and inputs share one
namespace.

`build` chooses the operation. A plan with write stages answers
`{"kind":"write","returns":"matched"}`, or `"rows"` when `returning` named
columns; a read answers `rows`, or `count`/`exists` with empty output
columns. Every method is infallible: the first invalid call is kept and
returned by `build`, which then runs full plan validation. Builder codes are
`query_plan_builder_duplicate_name`, `query_plan_builder_binding_limit`,
`query_plan_builder_untyped_relation` (`links` needs a relation bound by
`exact`/`subtypes`), `query_plan_builder_not_created` (fields and players on
a thing that was not inserted or put) and `query_plan_builder_write_operation`
(a write plan that asked for count or exists). The builder has no attach,
players-deletion, iid or fetch call; such plans are constructed as a
`PlanSpec` and validated by `QueryPlanV3::new`.

## TypeQL lowering

Plain plans lower their stages into one pipeline. Opt-in deletion snapshots
lower to a capture statement followed by a deletion statement in the same
write transaction. Both use the same read-only prefix and given rows. Capture
is bounded and schema-validated before deletion; any failure aborts the staged
stack or makes a borrowed transaction rollback-only. Snapshot role expansion
uses optional matches so it cannot silently remove a deletion target; required
role evidence is still validated before mutation.

Generate hygienic variables, resolve labels through checked identities and bind
values. The forms below describe the shared Rust lowering.

| Construct | TypeQL form |
| --- | --- |
| isa / has / links | `$x isa account;` / `$x has score $s;` / `$r links (member: $x);` |
| value | `$s >= $minimum;` and corresponding comparator symbols |
| or / not / try | `{ ... } or { ... };` / `not { ... };` / `try { ... };` |
| let | `let $next = $stock - $one;` |
| iid | `$x iid 0x1e00000000000000000001;` |
| arithmetic | Parenthesized `+`, `-`, `*`, `/`, `%`, `^` expressions |
| builtins | Same names except length becomes `len($text)` |
| insert / put | `insert $o isa order, has order-id $id;` / `put $o isa! order, has order-id $id;` (put matches the exact declared type) |
| field / role update | `update $x has score == $next;` / `update $r links (member: $new);` |
| attach | `insert $x has tag == $value;` / `insert $r links (member: $p);` over bound variables |
| delete thing / ownership / link | `delete $x;` / `delete has $old_score of $x;` / `delete links (member: $old) of $r;` |
| fetch then delete | Execute `<prefix>; <fetch>;` followed by `<prefix>; delete $x;`, retaining the validated capture as the answer. |
| delete players | `try { $r links (member: $p); };` then `delete try { links (member: $p) of $r; };` |
| select / require / distinct | `select $x;` / `require $x;` / `distinct;` |
| fetch | `match $x isa! $T; $x links (member: $P0); $P0 isa! $P0T;` (optional roles inside `try`), then `fetch { "_iid": iid($x), "_type": label($T), "attributes": { $x.* }, "_role_0_iid": iid($P0), "_role_0_type": label($P0T), "_role_0_attributes": { $P0.* } };` |
| reduce | `reduce $n = count($x) groupby $group;` |
| sort / offset / limit | `sort $x asc;` / `offset 10;` / `limit 10;` |
| output / operation | Resolve the declared rows, documents, count, presence or write result from that pipeline |

Ownership deletion first binds every value of the ownership (for example,
`$x has score $old_score`) in a preceding optional (`try`) match of that same
pipeline, then deletes that ownership using the attribute variable inside an
optional delete. It does not render a field label as a deletable. Owners
without a value keep their row, so later stages and `matched` still see them;
each additional value of a multi-valued ownership adds one row. A players
deletion binds and deletes every player of the role the same way.

Bounded reachability uses a checked local function over declared roles; function
calls use checked let assignments and definitions. Given rows seed that same
pipeline. A touch assigns the matched value back to its field, retaining a
write conflict edge. Relation reassignment deletes the old role link before
assigning the new player. Write-row projection follows mutation in that same
pipeline. The ORM executor (`type-bridge-core/crates/orm/src/query_plan_exec.rs`)
implements this lowering. Its deletion and fetch helper variables
(`PlanHas{stage}x{index}`, `PlanPlayers{stage}x{index}`, `PlanFetchType`,
`PlanFetchRole{N}`) start uppercase, so they cannot collide with validated
lowercase plan variables.

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

New constructs may extend `query-plan/v3` in place when each is fenced by a
required capability. Plans that do not use an added construct retain their
canonical bytes and fingerprints. A receiver lacking that capability refuses
admission; unknown syntax is rejected during decoding. Changes to existing
construct semantics, canonical encoding or numeric promotion still require a
new format and canonicalization version.

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

## Staged remote envelope V3

A staged transaction uses `typebridge.query-remote-request/v3`, independently
of the plan version. Requests remain unsigned. Write execution requires an
authenticated transport identity or explicit server policy; possessing an
advertisement or reply-signing public key does not authorize a write. This
section defines the codec. The standalone server executes it at
`POST /v3/staged` and advertises the staged capabilities at
`GET /v3/capabilities`; requests bind to that advertisement, not to the
unchanged `/v2/capabilities` one. A write transaction opens only when an
interceptor vouches for the caller (`Interceptor::on_v3_write_authority`
answers `StagedWriteAuthority::Authenticated`) or `[v2]` sets
`allow_unauthenticated_staged_writes = true`; otherwise the signed reply fails
with `staged_write_not_authorized`. Before each plan stages,
`Interceptor::on_v3_plan` sees its index, required capabilities, touched
types and operation, but no row values unless the policy returns `true` from
`wants_v3_plan_rows`; a refusal fails the signed reply with
`query_remote_policy_rejected`, stages nothing further and commits nothing.
Once the outcome is known, the observe-only `Interceptor::on_v3_commit` sees
`committed`, the diagnostic code, the commit certainty and the typed provider
failure (such as an `STC2` conflict). Staged replies carry fetch documents with the same
shape as direct answers. Rust validates exact types, inherited ownership,
attribute value domains and cardinalities, and required/optional role evidence
before commit and again when verifying the signed reply. Unknown keys, types,
attributes, values and roles fail with `query_remote_v3_document_key`,
`query_remote_v3_document_type`, `query_remote_v3_document_attribute`,
`query_remote_v3_document_value` and `query_remote_v3_document_role`.

Wire attributes are nested `RemoteFieldValue::Document` maps; each attribute
contains a canonical scalar or list according to its ownership cardinality
and collection mode. SDK documents expose ordinary
nested values. Optional players may omit their triple or carry three explicit
absences. Partial triples are refused. Execution charges each row/document
against the shared `AnswerLimits` policy while reading, tightened by the
request's `max_items`, `max_bytes` and deadline. Item and byte overflows report
`processed_item_limit` and `response_byte_limit` on direct and staged paths.
Once admitted, a request
finishes in a detached task: a client that disconnects mid-commit still has
its outcome committed and observed.

### Request

| Member | Wire shape |
| --- | --- |
| `format` | `typebridge.query-remote-request/v3` |
| `advertisement` | Lowercase 64-hex digest of the exact executor advertisement |
| `prepared_at_unix_ms` | Unsigned Unix millisecond timestamp |
| `expires_at_unix_ms` | Preparation time plus the selected deadline |
| `nonce` | 16..128 ASCII letters, digits, hyphens or underscores |
| `kind` | Optional `read`, `write` or `schema` |
| `limits` | Complete budget object below |
| `plans` | Nonempty ordered array of `{"plan_bytes":"...","rows":[...]}` |

Omit an absent `kind`; explicit null is not canonical. Each
`plan_bytes` is the lowercase hexadecimal encoding of exact canonical V3 plan
bytes. It is opaque bytes, not TypeQL or a host-reserialized plan object. Rows
are positional canonical scalar/null arrays under that plan's input columns.
A bound plan's rows must exactly equal the envelope rows. For an unbound
template, the envelope rows must satisfy its declared columns. A plan without
columns has `rows:[]`. Execution materializes the same validated values through
`given`; no value interpolation is permitted.

Kind defaults to write if any plan writes, otherwise read. Explicit read
refuses writes; explicit write permits reads. Schema refuses query plans. The
request is a stack, not an open transaction or a collection of separately
committed requests. Its answers resolve together after the transaction outcome.

Every request requires `query.remote.envelope-v3`, and every writing stack
adds `query.remote.write`.
The union of all plan capabilities is also required. The receiver checks every
plan under its own schema and the minimum of its query limits and the advertised
query limits. Sender metadata cannot widen receiver limits.

Deadline omission means 30,000 ms; explicit deadlines range from 1 to 300,000 ms.
Expiry equals preparation plus that duration, with checked arithmetic. Execution
must reject an expired request or a preparation timestamp more than 60,000 ms
in the future. Archival decoding is separate from this wall-clock freshness
check.

### Aggregate budgets

The complete `limits` object always includes these fields, including defaults:

| Member | Default | Supported range |
| --- | --- | --- |
| `deadline_ms` | null | null, or 1..300,000 |
| `max_bytes` | 33,554,432 | 1..33,554,432 signed successful reply bytes |
| `max_items` | 65,536 | 1..65,536 aggregate answer items |
| `max_staged_plans` | 64 | 1..64 |
| `max_read_statements` | 192 | 1..4,096 |
| `max_write_statements` | 256 | 1..4,096 |

Read and write provider statements are separate dimensions; the frozen V2
three-statement limit is unchanged. Request validation checks the minimum of
one statement per plan against the corresponding budget. Execution must charge
all actual statements, including any necessary split queries, before they fire
and refuse exceeding a budget inside the same transaction. The codec does not
execute or estimate an arbitrary provider's split strategy.

The request and signed reply retain the 32 MiB envelope ceiling. Embedded plans
retain their independent canonical plan ceilings, and each row batch retains
its 4,096-row/4 MiB input ceiling. Opaque plan-byte hex strings may use the
envelope budget; decoded plan strings and scalar values still retain their own
1 MiB ceiling. Hex framing consumes envelope bytes: a plan near its standalone
limit may not fit in an envelope. One answer row/document counts as an item;
count, exists and matched answers each count as one. A failed
reply does not consume the successful-data byte budget, allowing a small caller
budget to receive a diagnostic. Diagnostics remain capped at 65,536 bytes.

### Replies and failure vocabulary

The unsigned payload has required `format`, `nonce`, `request`, `outcome` and
`answers`. Its format is `typebridge.query-remote-response/v3`; `request` is the
exact request fingerprint digest, and the nonce must equal that request's. A successful outcome is
`{"kind":"committed"}`. A failed outcome is
`{"kind":"failed","diagnostic":SDK_DIAGNOSTIC_JSON}` and has `answers:[]`.
No partial answers escape a rejected or indeterminate transaction. A successful
read transaction is closed, with `committed` denoting a final successful stack;
execution does not try to commit a TypeDB read transaction.

The public authoring outcome is non-exhaustive and uses the existing
`SdkExecutionDiagnostic`, not a second failure model. Failure details follow the
SDK diagnostic vocabulary: version, category, sdk_category, query_category,
code, message, path and typed details. Typed failure details (`retry_safe`,
`phase`, `code`, `rejection`, `lock_kind`, `annotation`, and the staged index in
the path) join this same vocabulary when present; the released codes stay as
they are, and bindings whose released ABI fixes the detail set (C ABI 1.6) do
not see them. The client rebuilds the typed failure from these signed details
only when they name a complete, consistent failure in the closed vocabulary, so
a refusal over the proxy is typed exactly as the same direct commit would be.
`commit_outcome_unknown` remains indeterminate; a failed outcome alone never
implies a safe retry. SDK messages stay implementation-owned static text during
authoring. Authentication precedes interpretation of their bounded host-JSON
wire projection, which does not weaken the in-memory SDK diagnostic contract.

Committed answers have exactly one entry per plan, in stack order:

| Answer | Shape |
| --- | --- |
| rows | `{"kind":"rows","rows":[[REMOTE_VALUE,...],...]}` |
| documents | `{"kind":"documents","documents":[{"field":REMOTE_FIELD_VALUE},...]}` |
| count | `{"kind":"count","value":1}` |
| exists | `{"kind":"exists","value":true}` |
| matched | `{"kind":"matched","value":2}` |

Read rows and write/rows select rows or documents according to their output;
count/exists require their corresponding answer. Write/matched requires matched.
Widths, document keys, instance identity/type shape, total items and byte budgets
are checked against the stack. Remote values retain V1's tagged thing
(`iid`, `type_id`), attribute (`type_id`, `value`), value (`value`) and absent
forms. Remote field values are tagged scalar (`value`), list (`values`) or
absent. Successful byte and failed-diagnostic byte ceilings are checked using
borrowed authenticated payload fragments before allocating owned evidence.

### Fingerprint and signature

The request uses the same framed SHA-256 algorithm as plan fingerprints, with
domain `typebridge.query.remote-request`, canonicalization
`typebridge.query-remote-request/v3`, no semantic profile and the complete
canonical request bytes. This binds all plans, rows, limits, times, kind,
advertisement and nonce.

Replies use the unchanged `typebridge.query-remote-signed-reply/v1` outer object:
`advertisement`, `format`, `key`, `key_id`, `payload`, `signature`. Advertisement
is its digest; key is 32-byte Ed25519 public-key hex; signature is 64-byte hex.
Key ID is SHA-256 of `UTF8("typebridge.query.remote-reply-key-id/v1")`, a zero
byte and the raw public key. Let U be the canonical outer object with signature
omitted, including the complete V3 payload. Sign the 32-byte SHA-256 digest of
`UTF8("typebridge.query.remote-reply-signature/v1") || 0x00 || U` using Ed25519.
The receiver supplies the trusted public key, checks its advertisement binding,
authenticates the outer envelope and only then interprets/correlates the V3
payload. Unknown fields/tags and noncanonical reconstruction are refused.

Rust's genuine Ed25519 golden fixtures are in
`type-bridge-core/crates/contract/tests/fixtures/query-remote-v3/`: advertisement,
request, request fingerprint, committed reply and failed reply. Existing V2
envelope and V2/V3 plan fixtures remain byte-for-byte unchanged.

### Replays and unknown outcomes

The server keeps no replies. `POST /v3/staged` authenticates the transport,
decodes against the advertisement, checks freshness, authorizes the write and
runs plan policy; it then reserves the request's nonce for the request's whole
lifetime, executes once, signs and replies. The same bytes again never execute
again: a read is refused with `query_remote_replay`, and a write answers
`commit_outcome_unknown`, because its first copy may have committed. Every
other signed refusal applied nothing.

A reply that is lost or fails verification leaves the outcome unknown; the
application reconciles by reading the database, as after a lost direct commit.
A server restart forgets spent nonces, and its executor epoch and
advertisement change, so a request bound to the old advertisement is refused.
TypeBridge reports the outcome; retries, locks and concurrency limits are the
application's policy.
