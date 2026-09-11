import assert from "node:assert/strict";
import { Aliases, Archive, Marker, Name, NameValue } from "./generated_blockers/dist/index.js";

const name = Name.create("marker");
const values = { name_: name, nameValue: NameValue.create("label"), aliases: [], revision: null };
const marker = Marker.create(values);
assert.equal(marker.name_, name);
assert.equal(Marker.name, "Marker");
assert.equal(Marker.name_.multiplicity.cardinality.max, "1");
assert.equal(Marker.aliases.multiplicity.cardinality.max, "unbounded");
assert.equal(Marker.create({ ...values, aliases: Array(5).fill(Aliases.create("alias")) }).aliases.length, 5);
const reference = Marker.reference("marker-iid", { name_: name });
assert.equal(reference.name_, name);
assert.equal(reference.iid, "marker-iid");
assert.equal(Marker.reference(null, { name_: name }).name_, name);
const optional = Marker.create({ name_: name, nameValue: values.nameValue });
assert.deepEqual(optional.aliases, []);
assert.equal(optional.revision, null);
const roles = { name_: [marker, marker, marker], nameValue: [], reference_: [] };
assert.equal(Archive.create(roles).name_.length, 3);
assert.throws(() => Archive.create({ ...roles, name_: [] }), RangeError);
assert.throws(() => Archive.create({ ...roles, reference_: [marker, marker, marker] }), RangeError);
assert.equal(Archive.create({ ...roles, reference_: [marker, marker] }).reference_.length, 2);
assert.throws(() => Marker.create({ ...values, name_: null }), RangeError);
const hydrateComplete = Object.getOwnPropertySymbols(Marker).find(
  (symbol) => symbol.description === "typebridge.hydrate-complete",
);
assert(hydrateComplete);
const hydrated = Marker[hydrateComplete]("marker-iid", values);
assert.equal(hydrated.name_, name);
assert.equal(hydrated.iid, "marker-iid");
const hydratedArchive = Archive[hydrateComplete]("archive-iid", {
  name_: [hydrated, hydrated, hydrated], nameValue: [], reference_: [],
});
assert.equal(hydratedArchive.name_.length, 3);
assert.equal(hydratedArchive.name_[0].name_, name);
assert(Object.isFrozen(hydratedArchive));
console.log("TypeScript generation blocker regressions passed");
