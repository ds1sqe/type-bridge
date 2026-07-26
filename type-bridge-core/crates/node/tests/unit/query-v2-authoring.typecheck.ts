import {
  AuthoredQueryPlan,
  QueryPlanBuilder,
  QueryV2Authority,
} from "../../typescript/query-v2.js";

function completeVocabulary(authority: QueryV2Authority): AuthoredQueryPlan {
  const builder = new QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const friend = builder.binding("friend");
  const name = builder.binding("name");
  const aggregate = builder.binding("aggregate");
  const input = builder.input("prefix", "string", true);

  const personOperand = builder.bindingOperand(person);
  const nameOperand = builder.bindingOperand(name);
  const inputOperand = builder.inputOperand(input);
  const text = builder.literalOperand("string", "Ada");
  const long = builder.literalOperand("long", 1n);
  const double = builder.literalOperand("double", 1);
  const boolean = builder.literalOperand("boolean", true);
  const date = builder.literalOperand("date", "2026-07-24");
  const datetime = builder.literalOperand("datetime", "2026-07-24T00:00:00");
  const datetimeTz = builder.literalOperand("datetime_tz", "2026-07-24T00:00:00Z");
  const decimal = builder.literalOperand("decimal", "1.25");
  const duration = builder.literalOperand("duration", "P1DT2H");
  void [
    personOperand,
    long,
    double,
    boolean,
    date,
    datetime,
    datetimeTz,
    decimal,
    duration,
  ];

  const personIsa = builder.isa(person, "entity", "person", true);
  const friendIsa = builder.isa(friend, "entity", "person", true);
  const hasName = builder.has(person, name, "name");
  const links = builder.links(
    builder.binding("friendship"),
    "friendship",
    ["friend", "friend"],
    [person, friend],
  );
  builder.isa(builder.binding("relation_kind"), "relation", "friendship", false);
  builder.isa(builder.binding("attribute_kind"), "attribute", "name", false);
  const equals = builder.value("equal", nameOperand, text);
  builder.value("not_equal", nameOperand, text);
  builder.value("less", nameOperand, text);
  builder.value("less_or_equal", nameOperand, text);
  builder.value("greater", nameOperand, text);
  builder.value("greater_or_equal", nameOperand, text);
  const inputEquals = builder.value("equal", nameOperand, inputOperand);
  const negated = builder.not([inputEquals]);
  const alternative = builder.or([[friendIsa], [links]]);
  const optional = builder.try([friendIsa]);
  const reachable = builder.reachable(
    person,
    friend,
    "friendship",
    "friend",
    "friend",
    0,
    3,
  );
  const schemaCall = builder.functionCall(aggregate, [nameOperand], "score");
  void [negated, alternative, optional, reachable, schemaCall];

  builder.match([personIsa, hasName, equals]);
  builder.select([person, name]);
  builder.require([person, name]);
  builder.distinct();
  const count = builder.reduceAssignment(aggregate, "count");
  builder.reduceAssignment(aggregate, "max", name);
  builder.reduceAssignment(aggregate, "mean", name);
  builder.reduceAssignment(aggregate, "min", name);
  builder.reduceAssignment(aggregate, "sum", name);
  builder.reduce([count], [person]);
  const ascending = builder.order(person, "ascending");
  builder.order(person, "descending");
  builder.sort([ascending]);
  builder.offset(0n);
  builder.limit(10n);
  return builder.finalizeRows([person, aggregate]);
}

function localFunctionAndDocuments(authority: QueryV2Authority): AuthoredQueryPlan {
  const builder = new QueryPlanBuilder(authority);
  const localPerson = builder.binding("local_person");
  const localName = builder.binding("local_name");
  const localBody = [
    builder.isa(localPerson, "entity", "person", false),
    builder.has(localPerson, localName, "name"),
  ];
  const localReturn = builder.localReturn("sum", localName, "long");
  builder.localReturn("count", localName, "long");
  builder.localReturn("sum", localName, "double");
  const local = builder.localFunction(
    "local_score",
    [localPerson, localName],
    [localPerson],
    ["person"],
    localBody,
    localReturn,
  );

  const person = builder.binding("person");
  const score = builder.binding("score");
  const personOperand = builder.bindingOperand(person);
  const call = builder.functionCall(score, [personOperand], null, local);
  builder.match([
    builder.isa(person, "entity", "person", false),
    call,
  ]);
  const scalar = builder.documentBinding("score", score);
  const names = builder.documentAttributeList("names", person, "name");
  return builder.finalizeDocuments([scalar, names]);
}

declare const authority: QueryV2Authority;
const authored: AuthoredQueryPlan = completeVocabulary(authority);
const documents: AuthoredQueryPlan = localFunctionAndDocuments(authority);
const bytes: Uint8Array = authored.canonicalBytes;
const capabilities: readonly string[] = authored.requiredCapabilities;
const rows = authored.rows([[null]]);
const documentRows = documents.documents([]);
const countRows = authored.count([]);
const exists = authored.exists([]);
void [bytes, capabilities, rows, documentRows, countRows, exists];

const negative = new QueryPlanBuilder(authority);
const binding = negative.binding("value");
const negativeOperand = negative.bindingOperand(binding);
const negativeReturn = negative.localReturn("count", binding, "long");
const negativeLocal = negative.localFunction(
  "local",
  [binding],
  [binding],
  ["value"],
  [negative.value("equal", negativeOperand, negativeOperand)],
  negativeReturn,
);
// @ts-expect-error structs are value records, not queryable Isa types
negative.isa(binding, "struct", "record", false);
// @ts-expect-error long literals use lossless bigint, never Number
negative.literalOperand("long", 1);
// @ts-expect-error boolean literals do not accept numeric values
negative.literalOperand("boolean", 1);
// @ts-expect-error a function target is mandatory
negative.functionCall(binding, []);
// @ts-expect-error schema and local function targets are mutually exclusive
negative.functionCall(binding, [], "schema_function", negativeLocal);
// @ts-expect-error count never accepts an input binding
negative.reduceAssignment(binding, "count", binding);
// @ts-expect-error input reducers require an input binding
negative.reduceAssignment(binding, "sum");
// @ts-expect-error local max is not a total local-function return
negative.localReturn("max", binding, "long");
// @ts-expect-error local count returns long only
negative.localReturn("count", binding, "double");
// @ts-expect-error local sum returns a numeric scalar only
negative.localReturn("sum", binding, "boolean");
// @ts-expect-error authored plans are produced only by builder finalization
new AuthoredQueryPlan();
