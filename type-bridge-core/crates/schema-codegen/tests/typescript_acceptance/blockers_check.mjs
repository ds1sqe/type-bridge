import { Iid } from "@type-bridge/node";
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
const reference = Marker.reference(new Iid("0xa1"), { name_: name });
assert.equal(reference.name_, name);
assert.equal(reference.iid.toString(), "0xa1");
assert.equal(Marker.reference(null, { name_: name }).name_, name);
const optional = Marker.create({ name_: name, nameValue: values.nameValue });
assert.deepEqual(optional.aliases, []);
assert.equal(optional.revision, null);
const roles = { name_: [marker, marker, marker], nameValue: [], reference_: [] };
assert.equal(Archive.create(roles).name_.length, 3);
assert.throws(() => Archive.create({ ...roles, name_: [] }), { code: "missing_required_role" });
assert.throws(() => Archive.create({ ...roles, reference_: [marker, marker, marker] }), { code: "role_cardinality_violation" });
assert.equal(Archive.create({ ...roles, reference_: [marker, marker] }).reference_.length, 2);
assert.throws(() => Marker.create({ ...values, name_: null }), { code: "missing_required_field" });
const hydrateComplete = Object.getOwnPropertySymbols(Marker).find(
  (symbol) => symbol.description === "typebridge.hydrate-complete",
);
assert(hydrateComplete);
const hydrated = Marker[hydrateComplete]("0xa1", values);
assert.equal(hydrated.name_, name);
assert.equal(hydrated.iid.toString(), "0xa1");
const hydratedArchive = Archive[hydrateComplete]("0xb1", {
  name_: [hydrated, hydrated, hydrated], nameValue: [], reference_: [],
});
assert.equal(hydratedArchive.iid.toString(), "0xb1");
assert.equal(hydratedArchive.name_.length, 3);
assert.equal(hydratedArchive.name_[0].name_, name);
assert(Object.isFrozen(hydratedArchive));
console.log("TypeScript generation blocker regressions passed");

const { Powertrain, PowertrainReferenceValue } = await import("./generated_blockers/dist/index.js");
const engineValue = PowertrainReferenceValue.create("engine-1");
const engine = Powertrain.create({ powertrainRef: engineValue });
const engineReference = Powertrain.reference(new Iid("0x501"), { powertrainRef: engineValue });
assert.equal(engine.powertrainRef.value, "engine-1");
assert.equal(engineReference.powertrainRef.value, "engine-1");
const engineHydrator = Object.getOwnPropertySymbols(Powertrain).find(
  (symbol) => symbol.description === "typebridge.hydrate-complete",
);
assert(engineHydrator);
const hydratedEngine = Powertrain[engineHydrator]("0x501", { powertrainRef: engineValue });
assert.equal(hydratedEngine.powertrainRef.value, "engine-1");
assert.equal(hydratedEngine.iid.toString(), "0x501");
const engineBytes = Powertrain.encodeCreate(engine);
const engineJson = new TextDecoder().decode(engineBytes);
assert(engineJson.includes('"powertrain_ref"'));
assert(!engineJson.includes("PowertrainReferenceValue"));
assert.equal(Powertrain.decodeCreate(engineBytes).powertrainRef.value, "engine-1");
const engineReferenceBytes = Powertrain.encodeReference(engineReference);
assert.equal(Powertrain.decodeReference(engineReferenceBytes).powertrainRef.value, "engine-1");
