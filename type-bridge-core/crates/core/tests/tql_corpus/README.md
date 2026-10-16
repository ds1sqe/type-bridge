# Official TypeQL AST corpus

Every construct reachable in typeql 3.13.4 parses; every construct renders
except the listed upstream defects, which fail with a typed error.

`manifest.json` maps the 136 audited construct rows to query files. The
three legacy Raw escape hatches and trailing-comma nicety are excluded.
Files use the official plain Display format; tests compare the parsed
upstream AST before and after rendering, including source positions.
Additional files cover namespaced calls, schema renames and redefine sequences.

Known upstream gaps (all from the crates.io typeql 3.13.4 source):

| File | Behavior | Upstream evidence |
| --- | --- | --- |
| `struct-destructuring.tql` (row 29) | Parses; render returns `Unsupported { StructDestructuring }` before the upstream panic. | `src/statement/mod.rs:151`: `AssignmentPattern::Deconstruct` calls `todo!()`. |
| `end.tql` (row 4) | Parses; render returns `Unsupported { ExplicitEnd }`. | `src/query/mod.rs:47`: explicit `end` loses its semicolon. |
| `schema-rename.tql` | Parses; render returns `Unsupported { SchemaRename }`. | `src/schema/definable/mod.rs`: rename emits a semicolon, and `src/query/schema/redefine.rs` appends another. |
| `schema-redefine-sequence.tql` | Parses; render returns `Unsupported { RedefineSequence }`. | `src/query/schema/redefine.rs` joins semicolon-terminated definables with another semicolon. |
| `struct-literal.tql` (negative expression case) | Both official and adapter parse reject `let $x = {field: 1}`. | `src/parser/typeql.pest:149–152` omit structs from expressions. The struct rule at line 170 is reachable through `isa_constraint` at line 112; `struct-instance.tql` covers that positive form (row 91). |

Plain Display is intentional: the upstream Pretty implementation renders
optional `try` patterns as `not` (`src/pattern/mod.rs:108`). The adapter
never selects Pretty. A separate builder regression checks the optional
pattern's meaning and exact decimal and escaped-string representation.

These exceptions were approved for the 2.3 adapter contract. They are
retained explicitly rather than hidden by a second parser or renderer.

Named-zone datetime-tz literals use one compatibility formatter inside the
adapter. Upstream `src/value.rs:311–315` drops the space before the IANA
zone. The adapter prepares that literal's lexical separator for Display;
all other nodes use official Display unchanged. Remove this compatibility
function when upstream fixes the defect. The IANA fixture and builder
regression reparse the output and verify the same literal value and zone.
User input values should use `given` rows.

The `live/` corpus is a representative executable schema and nine pipeline
stages, verified through the real runtime on TypeDB 3.13.6. The top-level
grammar fixtures also include syntactic forms requiring their own schema,
functions or input bindings; they are parser/AST coverage fixtures.
