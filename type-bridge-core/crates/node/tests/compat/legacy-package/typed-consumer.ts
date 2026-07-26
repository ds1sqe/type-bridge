import {
  Entity,
  Key,
  QueryV2Error,
  Relation,
  attr,
  field,
  loadNative,
  queryV2ExecuteLocal,
  queryV2PrepareRemote,
  queryV2RemoteCapabilities,
  role,
  type EntityDescriptor,
  type ModelClass,
  type ModelInstance,
  type ParentModelClass,
  type PendingQueryV2Remote,
  type QueryV2RemoteLimits,
  type ResolvedTypeFlags,
  type RustDatabase,
  type RustDatabaseConnectOptions,
  type RustTransactionContext,
} from "@type-bridge/node";
import {
  AuthoredQueryInvocation,
  AuthoredQueryPlan,
  QueryPlanBuilder,
  QueryV2Authority,
} from "@type-bridge/node/query-v2";
import {
  QuerySession,
  references,
  type Page,
  type Query,
} from "@type-bridge/node/typed";

class PackedName extends attr.String("packed-typed-name") {}
class PackedActive extends attr.Boolean("packed-typed-active") {}
class PackedPerson extends Entity("packed-typed-person", {
  name: field(PackedName, Key),
  active: field(PackedActive),
}) {}
class PackedCompany extends Entity("packed-typed-company", {
  name: field(PackedName, Key),
}) {}
class PackedEmployment extends Relation("packed-typed-employment", {
  name: field(PackedName, Key),
  employee: role(PackedPerson),
  employer: role(PackedCompany),
}) {}
class PackedEnvelope extends Relation("packed-typed-envelope", {
  nested: role(PackedEmployment),
}) {}

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
    (<Value>() => Value extends Right ? 1 : 2)
    ? (<Value>() => Value extends Right ? 1 : 2) extends
        (<Value>() => Value extends Left ? 1 : 2)
      ? true
      : false
    : false;
type Expect<Condition extends true> = Condition;

const secureConnectOptions: RustDatabaseConnectOptions = {
  tlsEnabled: true,
  tlsRootCa: "/run/secrets/type-db-root.pem",
};
void secureConnectOptions;

function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function assertPackedStaticSurface(
  database: RustDatabase,
  transaction: RustTransactionContext,
  typeNameOrFlags: string | ResolvedTypeFlags,
): void {
  const databaseSession = new QuerySession(database);
  const transactionSession = new QuerySession(transaction);
  const packedPerson = databaseSession.var(PackedPerson);
  const packedCompany = databaseSession.var(PackedCompany);
  const packedEmployment = databaseSession.var(PackedEmployment);
  const packedEnvelope = databaseSession.var(PackedEnvelope);
  const personReferences = references(PackedPerson);
  const companyReferences = references(PackedCompany);
  const employmentReferences = references(PackedEmployment);
  const envelopeReferences = references(PackedEnvelope);

  const UnionEntity = Entity(typeNameOrFlags, {});
  const UnionRelation = Relation(typeNameOrFlags, {});
  const UnionChildEntity = Entity(typeNameOrFlags, {}, { parent: PackedPerson });
  const UnionChildRelation = Relation(typeNameOrFlags, {}, { parent: PackedEmployment });

  const exactPage: Page<PackedPerson> = databaseSession
    .query(packedPerson)
    .pageBy(packedPerson, { limit: 10, includeTotal: true });
  const exactTotal: bigint | undefined = exactPage.total;

  type PackedWork = Readonly<{
    person: PackedPerson;
    employments: readonly PackedEmployment[];
    companies: readonly PackedCompany[];
  }>;
  const workPage: Page<PackedWork> = databaseSession
    .queryNamed({
      person: packedPerson,
      employments: packedEmployment.collect(),
      companies: packedCompany.collect().distinct(),
    })
    .pageBy(packedPerson, { limit: 10 });

  const relationPlayerRow = databaseSession
    .queryNamed({ envelope: packedEnvelope, selected: packedEmployment })
    .where(
      packedEnvelope
        .role(envelopeReferences.roles.nested)
        .connects(packedEmployment),
    )
    .one();
  type SelectedEmployment = typeof relationPlayerRow.selected;
  type ShallowEmployment = Exclude<
    (typeof relationPlayerRow.envelope)["nested"],
    readonly unknown[]
  >;
  type SelectedEmployeeIsComplete = Expect<
    Equal<SelectedEmployment["employee"], PackedPerson | readonly PackedPerson[]>
  >;
  type SelectedEmployerIsComplete = Expect<
    Equal<SelectedEmployment["employer"], PackedCompany | readonly PackedCompany[]>
  >;
  type ShallowEmployeeIsAbsent = Expect<Equal<ShallowEmployment["employee"], undefined>>;
  type ShallowEmployerIsAbsent = Expect<Equal<ShallowEmployment["employer"], undefined>>;
  type ShallowOwnedKeyIsPresent = Expect<Equal<ShallowEmployment["name"], PackedName>>;
  type ShallowIidIsPresent = Expect<Equal<ShallowEmployment["_iid"], string | null>>;

  const single = databaseSession.query(packedPerson).where(
    packedPerson.field(personReferences.fields.name).startsWith("A"),
  );
  const pair = databaseSession.query(packedPerson, packedCompany);
  const named = databaseSession.queryNamed({
    person: packedPerson,
    companies: packedCompany.collect().distinct(),
  });

  type SingleRow = ReturnType<(typeof single)["one"]>;
  type PairRow = ReturnType<(typeof pair)["one"]>;
  type NamedRow = ReturnType<(typeof named)["one"]>;
  type SingleInferenceIsExact = Expect<Equal<SingleRow, PackedPerson>>;
  type PairInferenceIsExact = Expect<
    Equal<PairRow, readonly [PackedPerson, PackedCompany]>
  >;
  type NamedInferenceIsExact = Expect<
    Equal<
      NamedRow,
      Readonly<{
        person: PackedPerson;
        companies: readonly PackedCompany[];
      }>
    >
  >;

  const exactPair: Query<readonly [PackedPerson, PackedCompany]> = pair;
  // @ts-expect-error a two-slot query must not widen or collapse to one slot
  const wrongPair: Query<readonly [PackedPerson]> = pair;

  // @ts-expect-error public typed sessions require an execution connection
  new QuerySession();
  // @ts-expect-error a plain object is not a supported query connection
  new QuerySession({});
  // @ts-expect-error same-shaped foreign field owners remain incompatible
  packedPerson.field(companyReferences.fields.name);
  // @ts-expect-error boolean fields expose equality only
  packedPerson.field(personReferences.fields.active).gt(new PackedActive(true));
  // @ts-expect-error employee accepts PackedPerson, not PackedCompany
  packedEmployment.role(employmentReferences.roles.employee).connects(packedCompany);
  // @ts-expect-error pages require exactly one singular root
  databaseSession.query(packedPerson, packedCompany).pageBy(packedPerson, { limit: 10 });

  void transactionSession;
  void exactPage;
  void exactTotal;
  void workPage;
  void relationPlayerRow;
  void UnionEntity;
  void UnionRelation;
  void UnionChildEntity;
  void UnionChildRelation;
  void exactPair;
  void wrongPair;
  void (true as SingleInferenceIsExact);
  void (true as PairInferenceIsExact);
  void (true as NamedInferenceIsExact);
}

void assertPackedStaticSurface;

function assertPackedPreparedV2Surface(
  database: RustDatabase,
  authority: QueryV2Authority,
  plan: Uint8Array,
  advertisement: Uint8Array,
  response: Uint8Array,
): void {
  const invocation = JSON.stringify({ operation: "rows", rows: [] });
  const limits: QueryV2RemoteLimits = {
    maxItems: 100n,
    maxBytes: 1_048_576n,
    maxCollectionMembers: 1_000n,
    deadlineMs: 30_000n,
  };
  const local: Promise<string> = queryV2ExecuteLocal(
    database,
    authority,
    plan,
    invocation,
    limits.deadlineMs,
  );
  const capabilities: readonly string[] = queryV2RemoteCapabilities(advertisement);
  const pending: PendingQueryV2Remote = queryV2PrepareRemote(
    authority,
    plan,
    invocation,
    advertisement,
    limits,
  );
  const request: Uint8Array = pending.requestBytes();
  const outcome: Promise<string> = pending.decodeReply(response);
  void local;
  void capabilities;
  void request;
  void outcome;
}

void assertPackedPreparedV2Surface;

const packedDeclaredText =
  '{"declared_identity":{"algorithm":"sha256","canonicalization":' +
  '"typebridge.schema-canonical-json/v1","digest":' +
  '"bdab7138a57238ee23dfceb69e7f09893cfa7b536d9e70356d1a986a132499fe",' +
  '"domain":"typebridge.schema.declared-identity"},"facts":[{"kind":"type",' +
  '"value":{"id":{"kind":"entity","label":"smoke-person"}}},{"kind":"type",' +
  '"value":{"id":{"kind":"attribute","label":"smoke-name"}}},{"kind":"value",' +
  '"value":{"id":"smoke-name","value_type":"string"}},{"kind":"owns","value":' +
  '{"id":{"attribute":"smoke-name","owner":{"kind":"entity","label":' +
  '"smoke-person"}}}}],"format_version":1,"required_capabilities":[]}';
const packedDeclaredSchema = Uint8Array.from(
  packedDeclaredText,
  (character) => character.charCodeAt(0),
);
const authoredAuthority = new QueryV2Authority(
  packedDeclaredSchema,
  "binding-smoke",
  "typedb-3.12.1/v1",
);
const authoredBuilder = new QueryPlanBuilder(authoredAuthority);
const authoredPerson = authoredBuilder.binding("person");
const authoredName = authoredBuilder.binding("name");
const authoredWanted = authoredBuilder.input("wanted_name", "string", false);
authoredBuilder.match([
  authoredBuilder.isa(authoredPerson, "entity", "smoke-person", true),
  authoredBuilder.has(authoredPerson, authoredName, "smoke-name"),
  authoredBuilder.value(
    "equal",
    authoredBuilder.bindingOperand(authoredName),
    authoredBuilder.inputOperand(authoredWanted),
  ),
]);
authoredBuilder.select([authoredPerson, authoredName]);
authoredBuilder.require([authoredName]);
authoredBuilder.distinct();
authoredBuilder.sort([authoredBuilder.order(authoredName, "ascending")]);
const authoredPlan: AuthoredQueryPlan = authoredBuilder.finalizeRows([
  authoredPerson,
  authoredName,
]);
const authoredInvocation: AuthoredQueryInvocation = authoredPlan.rows([["Alice"]]);
invariant(
  authoredPlan.format === "typebridge.query-plan/v2",
  "packed query-v2 facade must author the canonical V2 format",
);
invariant(
  authoredPlan.fingerprint.length === 64,
  "packed query-v2 facade must return a SHA-256 fingerprint",
);
invariant(
  authoredInvocation.planFingerprint === authoredPlan.fingerprint,
  "packed invocation must remain bound to its authored plan",
);
let finalizedError: unknown;
try {
  authoredBuilder.binding("after_finalize");
} catch (error) {
  finalizedError = error;
}
invariant(
  finalizedError instanceof QueryV2Error
    && finalizedError.code === "query_builder_finalized",
  "packed builder must preserve the shared structured finalized diagnostic",
);

function assertLegacyRootStructuralSurface(
  instanceShape: Readonly<{
    _iid: string | null;
    toDict(): {};
  }>,
  parentShape: (new (values: never) => object) & {
    readonly typeName: string;
    readonly schema: {};
  },
  modelShape: (new (values: {}) => ModelInstance<{}>) & Pick<
    ModelClass<{}, EntityDescriptor>,
    "typeName" | "schema" | "flags" | "descriptor" | "fromDict" | "manager"
  >,
): void {
  const instance: ModelInstance<{}> = instanceShape;
  const parent: ParentModelClass<{}> = parentShape;
  const model: ModelClass<{}, EntityDescriptor> = modelShape;
  void instance;
  void parent;
  void model;
}

void assertLegacyRootStructuralSurface;

const native = loadNative();
const registry = new native.NodeDescriptorRegistry();
registry.registerEntityJson(JSON.stringify(PackedPerson.descriptor()));
const nativeSession = new native.NodeMatchSessionHandle(registry);
const nativePerson = nativeSession.exact(PackedPerson.typeName);
const nativeQuery = nativeSession.query(nativeSession.positional([nativePerson.one()]));
const diagnostic = nativeQuery.fetchRowsDiagnostic([], 0n, 1n, "exactly_one");
invariant(
  native.revalidateMatchDiagnostic(registry, diagnostic) === diagnostic,
  "packed opaque query handles must round-trip a canonical diagnostic",
);
invariant(
  Object.keys(nativeQuery).length === 0,
  "packed native query handles must expose no semantic plan fields",
);

const runtimeNative = native as unknown as Record<string, unknown>;
const resultConstructor = runtimeNative["NodeValidatedMatchResultHandle"];
const thingConstructor = runtimeNative["NodeValidatedThingHandle"];
invariant(typeof resultConstructor === "function", "opaque result symbol must be packed");
invariant(typeof thingConstructor === "function", "opaque thing symbol must be packed");

const resultPrototype = (resultConstructor as { readonly prototype: object }).prototype;
const thingPrototype = (thingConstructor as { readonly prototype: object }).prototype;
invariant(!("result" in resultPrototype), "opaque result must not expose a whole-result DTO");
invariant(!("toJSON" in resultPrototype), "opaque result must not expose JSON serialization");
invariant(!("toJSON" in thingPrototype), "opaque thing must not expose JSON serialization");

for (const constructor of [resultConstructor, thingConstructor]) {
  let constructError: unknown;
  try {
    Reflect.construct(constructor as Function, []);
  } catch (error) {
    constructError = error;
  }
  invariant(constructError instanceof Error, "opaque native result symbols must be nonconstructible");
  invariant(
    constructError.message.includes("contains no `constructor`"),
    "opaque native result construction must fail closed",
  );
}
