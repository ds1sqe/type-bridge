import type { RustDatabase } from "@type-bridge/node";

import {
  type Page,
  type Query,
  QuerySession,
  references,
} from "@type-bridge/node/typed";
import { ContractName, ContractPerson } from "./models.js";

declare const database: RustDatabase;
const session = new QuerySession(database);

// @ts-expect-error a public query session requires an execution connection
new QuerySession();

// @ts-expect-error a query must select at least one output slot
type EmptyQuery = Query<readonly []>;
void (null as unknown as EmptyQuery);

type SeventeenSlots = readonly [
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
];
// @ts-expect-error the canonical selected-output cap is sixteen slots
type SeventeenSlotQuery = Query<SeventeenSlots>;
void (null as unknown as SeventeenSlotQuery);

// @ts-expect-error QuerySession.query requires one or more selections
session.query();

const p01 = session.var(ContractPerson);
const p02 = session.var(ContractPerson);
const p03 = session.var(ContractPerson);
const p04 = session.var(ContractPerson);
const p05 = session.var(ContractPerson);
const p06 = session.var(ContractPerson);
const p07 = session.var(ContractPerson);
const p08 = session.var(ContractPerson);
const p09 = session.var(ContractPerson);
const p10 = session.var(ContractPerson);
const p11 = session.var(ContractPerson);
const p12 = session.var(ContractPerson);
const p13 = session.var(ContractPerson);
const p14 = session.var(ContractPerson);
const p15 = session.var(ContractPerson);
const p16 = session.var(ContractPerson);
const p17 = session.var(ContractPerson);
const personRefs = references(ContractPerson);

// @ts-expect-error typed string fields expose regex(), not raw TypeQL like()
p01.field(personRefs.fields.name).like(new ContractName("A"));

const seventeenSelections = [
  p01,
  p02,
  p03,
  p04,
  p05,
  p06,
  p07,
  p08,
  p09,
  p10,
  p11,
  p12,
  p13,
  p14,
  p15,
  p16,
  p17,
] as const;
// @ts-expect-error QuerySession.query rejects a seventeenth selected slot
session.query(...seventeenSelections);

const single = session.query(p01);
// @ts-expect-error a one-slot query page contains ContractPerson, not a tuple row
const wrongPage: Page<readonly [ContractPerson]> = single.pageBy(p01, { limit: 10 });
void wrongPage;
