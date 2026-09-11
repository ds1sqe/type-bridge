import { Archive, Marker, Name, NameValue, type MarkerRef } from "./generated_blockers/src/index.js";

const name = Name.create("marker");
const marker: Marker = Marker.create({
  name_: name,
  nameValue: NameValue.create("label"),
  aliases: [],
  revision: null,
});
const reference: MarkerRef = Marker.reference("marker-iid", { name_: name });
const archive: Archive = Archive.create({
  name_: [marker, marker, marker],
  nameValue: [],
  reference_: [],
});
const key: Name = marker.name_;
void reference;
void archive;
void key;
// @ts-expect-error Physical labels do not replace canonical projected member names.
Marker.create({ name, nameValue: NameValue.create("label"), aliases: [], revision: null });
