import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");

type CorpusCase = Readonly<{
  id: string;
  expected: Readonly<{
    outcome: "success" | "error";
    error_category: string | null;
    error_code: string | null;
  }>;
}>;

const repositoryRoot = path.resolve(process.cwd(), "../../..");
const corpus = JSON.parse(
  fs.readFileSync(
    path.join(repositoryRoot, "tests/contracts/typed_query/corpus-v1.json"),
    "utf8",
  ),
) as Readonly<{ cases: readonly CorpusCase[] }>;
const results = JSON.parse(
  fs.readFileSync(
    path.join(repositoryRoot, "tests/contracts/typed_query/expected-results-v1.json"),
    "utf8",
  ),
) as Readonly<{
  fixture_id: string;
  expected: Readonly<Record<string, unknown>>;
}>;
const generatedProjection = JSON.parse(
  fs.readFileSync(
    path.join(
      repositoryRoot,
      "tests/contracts/typed_query/generated-operation-projection-v1.json",
    ),
    "utf8",
  ),
) as Readonly<Record<string, unknown>>;

const errors = new Map(
  corpus.cases
    .filter((entry) => entry.expected.outcome === "error")
    .map((entry) => [
      entry.id,
      [entry.expected.error_category, entry.expected.error_code] as const,
    ]),
);

export function corpusError(caseId: string): readonly [string, string] {
  const value = errors.get(caseId);
  assert.ok(value?.[0] != null && value[1] != null, `missing corpus error ${caseId}`);
  return [value[0], value[1]];
}

export function assertLiveProjectionMatchesManifest(): void {
  const expected = results.expected;
  assert.deepEqual(generatedProjection, {
    source_fixture: results.fixture_id,
    distinct_roots: expected.distinct_roots,
    page_by_person_offset_0_limit_1: expected.page_by_person_offset_0_limit_1,
    alice_collect_count: (expected.alice_collect_employments as readonly unknown[]).length,
    alice_collect_distinct_count: (
      expected.alice_collect_distinct_employments as readonly unknown[]
    ).length,
    count_by_person: expected.count_by_person,
    exists_by_person: expected.exists_by_person,
  });
}
