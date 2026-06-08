// Compile-only type-level checks for multi-value (list) attributes (Phase 2).
// These are NOT runtime tests — they exist purely to prove that the TS type
// system catches incorrect usage of list fields. The file is compiled by
// tsconfig.unit.json; failures appear as type errors, not test failures.

import { Card, Entity, Key, Unique, attr, field } from "../../typescript/index.js";

class TagAttr extends attr.String("parity-tag") {}
class IdAttr extends attr.String("parity-id") {}
class NameAttr extends attr.String("parity-name") {}
class ScoreAttr extends attr.Double("parity-score") {}

class Sample extends Entity("sample-entity", {
  id: field(IdAttr, Key),
  name: field(NameAttr, Unique),
  score: field(ScoreAttr).optional(),
  // Multi-value optional list (Card(0,5)): tags?: TagAttr[] | undefined
  tags: field(TagAttr).list(Card(0, 5)),
  // Multi-value required list (Card(1,3)): required_tags: TagAttr[]
  required_tags: field(TagAttr).list(Card(1, 3)),
}) {}

// --- Positive cases (must compile) ---

// Constructing with a TagAttr array for the list field.
const s1 = new Sample({
  id: new IdAttr("s1"),
  name: new NameAttr("alice"),
  tags: [new TagAttr("ts"), new TagAttr("js")],
  required_tags: [new TagAttr("required")],
});

// Constructing without the optional list field (Card(0,5) → optional).
const s2 = new Sample({
  id: new IdAttr("s2"),
  name: new NameAttr("bob"),
  required_tags: [new TagAttr("req")],
});

// Reading back: list field is TagAttr[] | undefined when optional.
const tags: TagAttr[] | undefined = s1.tags;
// Reading back: required list is TagAttr[] (no undefined).
const requiredTags: TagAttr[] = s1.required_tags;

void s1;
void s2;
void tags;
void requiredTags;

// --- Negative cases (must NOT compile) ---

new Sample({
  id: new IdAttr("bad"),
  name: new NameAttr("bad"),
  // @ts-expect-error scalar passed where list is required: TagAttr is not TagAttr[]
  tags: new TagAttr("scalar-not-array"),
  required_tags: [new TagAttr("ok")],
});

new Sample({
  id: new IdAttr("bad2"),
  name: new NameAttr("bad2"),
  // @ts-expect-error wrong-brand element in list: ScoreAttr is not TagAttr
  tags: [new ScoreAttr(1.5)],
  required_tags: [new TagAttr("ok")],
});

// @ts-expect-error required list field (Card(1,3)) must not be omitted
new Sample({
  id: new IdAttr("bad3"),
  name: new NameAttr("bad3"),
});

// @ts-expect-error wrong brand on list read: TagAttr[] is not assignable to ScoreAttr[]
const wrongBrandRead: ScoreAttr[] = s1.required_tags;
void wrongBrandRead;
