import type {
  AggregateInput,
  AttributeValue,
  DynamicComparisonOp,
  DynamicExpr,
  DynamicQuerySpec,
  DynamicSort,
  DynamicSortDir,
} from "./index.js";

/** Minimal field label consumed by the separately retained V1 group-by facade. */
export interface QueryGroupField {
  readonly attrName: string;
}

/** Raised when a Rust aggregate/group-by result does not match the documented shape. */
export class TypedQueryError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TypedQueryError";
  }
}

/**
 * A typed query-filter expression. Subclasses serialize to the wire `DynamicExpr`
 * tree that the Rust core lowers into a database query; the boolean combinators
 * (`and_`, `or_`, `not_`) build nested expressions so a whole filter tree is one
 * value.
 */
export abstract class QueryExpr {
  /** Serialize to the Rust `DynamicExpr` wire shape. */
  abstract toExpr(): DynamicExpr;

  and_(...others: QueryExpr[]): QueryExpr {
    return new BooleanExpr("and", [this, ...others]);
  }

  or_(...others: QueryExpr[]): QueryExpr {
    return new BooleanExpr("or", [this, ...others]);
  }

  not_(): QueryExpr {
    return new NotExpr(this);
  }
}

/**
 * Attribute comparison or string-operator leaf. String operators
 * (`starts_with`/`ends_with`) carry the raw literal — Rust owns anchoring and
 * escaping (Plan 09 Gap B).
 */
export class ComparisonExpr extends QueryExpr {
  constructor(
    private readonly attrName: string,
    private readonly operator: DynamicComparisonOp,
    private readonly value: AttributeValue,
  ) {
    super();
  }

  toExpr(): DynamicExpr {
    return { kind: "compare", attr_name: this.attrName, operator: this.operator, value: this.value };
  }
}

/** Conjunction/disjunction of child expressions. */
export class BooleanExpr extends QueryExpr {
  constructor(
    private readonly op: "and" | "or",
    private readonly children: QueryExpr[],
  ) {
    super();
  }

  toExpr(): DynamicExpr {
    const exprs = this.children.map((child) => child.toExpr());
    return this.op === "and" ? { kind: "and", exprs } : { kind: "or", exprs };
  }
}

/** Negation of a child expression. */
export class NotExpr extends QueryExpr {
  constructor(private readonly child: QueryExpr) {
    super();
  }

  toExpr(): DynamicExpr {
    return { kind: "not", expr: this.child.toExpr() };
  }
}

/** A typed sort key produced by `Attr.asc()` / `Attr.desc()`. */
export class SortExpr {
  constructor(
    private readonly attrName: string,
    private readonly direction: DynamicSortDir,
  ) {}

  toSort(): DynamicSort {
    return { kind: "attribute", attr_name: this.attrName, direction: this.direction };
  }
}

/**
 * A typed aggregate produced by `Attr.avg()` / `agg.count()` etc. `input.result_key`
 * is the wire reduce variable (must be a valid identifier — attribute names with
 * `-` are sanitized to `_`); `resultKey` is the user-facing key in the normalized
 * result, which preserves the original attribute name. The wire `function` is the
 * TypeDB reduce name (`mean` for `avg`).
 */
export class AggregateSpec {
  constructor(
    readonly input: AggregateInput,
    readonly resultKey: string,
  ) {}
}

/** Field-independent aggregate helpers. */
export const agg = {
  /** Count matching rows. Normalizes to the `count` result key. */
  count(): AggregateSpec {
    return new AggregateSpec({ result_key: "count", function: "count", attr_name: null }, "count");
  },
} as const;

type DynamicManagerLike<Row> = {
  query(spec: DynamicQuerySpec): Row[];
  queryCount(spec: DynamicQuerySpec): bigint;
  queryAggregate(spec: DynamicQuerySpec, aggregates: AggregateInput[]): unknown[];
  queryGroupByAggregate(spec: DynamicQuerySpec, groupFields: string[], aggregates: AggregateInput[]): unknown[];
};

/**
 * Builder for a typed query over one entity or relation manager. Filters, sort,
 * and pagination accumulate into a `DynamicQuerySpec`; terminal methods
 * (`all`/`first`/`count`/`exists`/`aggregate`/`groupBy`) execute through the Rust
 * dynamic query seam. Reads hydrate through the manager's `hydrate` callback,
 * preserving Plan 08 typed instances and `_iid`.
 */
export class TypedQuery<T, Row> {
  readonly #manager: DynamicManagerLike<Row>;
  readonly #hydrate: (rows: Row[]) => T[];
  readonly #exprs: DynamicExpr[] = [];
  readonly #sorts: DynamicSort[] = [];
  #limit: number | null = null;
  #offset: number | null = null;

  constructor(manager: DynamicManagerLike<Row>, hydrate: (rows: Row[]) => T[]) {
    this.#manager = manager;
    this.#hydrate = hydrate;
  }

  /** Add filter expressions. Multiple calls (and multiple args) are ANDed in Rust. */
  filter(...exprs: QueryExpr[]): this {
    for (const expr of exprs) {
      this.#exprs.push(expr.toExpr());
    }
    return this;
  }

  orderBy(...sorts: SortExpr[]): this {
    for (const sort of sorts) {
      this.#sorts.push(sort.toSort());
    }
    return this;
  }

  limit(limit: number): this {
    this.#limit = limit;
    return this;
  }

  offset(offset: number): this {
    this.#offset = offset;
    return this;
  }

  all(): T[] {
    return this.#hydrate(this.#manager.query(this.#spec()));
  }

  execute(): T[] {
    return this.all();
  }

  first(): T | null {
    const rows = this.#manager.query({ ...this.#spec(), limit: 1 });
    return this.#hydrate(rows)[0] ?? null;
  }

  count(): bigint {
    return this.#manager.queryCount({ expr: [...this.#exprs] });
  }

  exists(): boolean {
    return this.count() > 0n;
  }

  /** Reduce matching rows to one normalized result object keyed by aggregate result keys. */
  aggregate(...aggregates: AggregateSpec[]): Record<string, unknown> {
    const rows = this.#manager.queryAggregate(
      { expr: [...this.#exprs] },
      aggregates.map((a) => a.input),
    ) as Record<string, unknown>[];
    if (rows.length !== 1) {
      throw new TypedQueryError(`Expected exactly one aggregate row, got ${rows.length}`);
    }
    return normalizeAggregateRow(rows[0], aggregates);
  }

  groupBy(...attrs: QueryGroupField[]): TypedGroupByQuery<Row> {
    return new TypedGroupByQuery(this.#manager, [...this.#exprs], attrs);
  }

  #spec(): DynamicQuerySpec {
    return {
      expr: [...this.#exprs],
      sort: [...this.#sorts],
      limit: this.#limit,
      offset: this.#offset,
    };
  }
}

/**
 * Grouped-aggregate query produced by `TypedQuery.groupBy(...)`. `aggregate(...)`
 * delegates to the Rust group-by seam and normalizes each group row into an
 * object keyed by group-attribute names plus aggregate result keys.
 */
export class TypedGroupByQuery<Row> {
  readonly #manager: DynamicManagerLike<Row>;
  readonly #exprs: DynamicExpr[];
  readonly #groupAttrs: QueryGroupField[];

  constructor(
    manager: DynamicManagerLike<Row>,
    exprs: DynamicExpr[],
    groupAttrs: QueryGroupField[],
  ) {
    this.#manager = manager;
    this.#exprs = exprs;
    this.#groupAttrs = groupAttrs;
  }

  aggregate(...aggregates: AggregateSpec[]): Record<string, unknown>[] {
    const groupNames = this.#groupAttrs.map((attr) => attr.attrName);
    const rows = this.#manager.queryGroupByAggregate(
      { expr: [...this.#exprs] },
      groupNames,
      aggregates.map((a) => a.input),
    ) as Record<string, unknown>[];
    return rows.map((row) => normalizeGroupRow(row, groupNames, aggregates));
  }
}

function normalizeAggregateRow(
  row: Record<string, unknown>,
  aggregates: AggregateSpec[],
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const aggregate of aggregates) {
    out[aggregate.resultKey] = unwrapAggregateValue(row, aggregate.input.result_key, aggregate.resultKey);
  }
  return out;
}

function normalizeGroupRow(
  row: Record<string, unknown>,
  groupNames: string[],
  aggregates: AggregateSpec[],
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  groupNames.forEach((name, index) => {
    out[name] = unwrapAggregateValue(row, `group${index}`, name);
  });
  for (const aggregate of aggregates) {
    out[aggregate.resultKey] = unwrapAggregateValue(row, aggregate.input.result_key, aggregate.resultKey);
  }
  return out;
}

// Rust returns reduce rows as TypeDB concept documents: keys are the reduce
// variables (`count`, `group0`, ...) and each value is a concept document whose
// `value` field holds the scalar. Unwrap both layers, failing loudly on any
// other shape.
function unwrapAggregateValue(row: Record<string, unknown>, wireKey: string, label: string): unknown {
  if (!(wireKey in row)) {
    throw new TypedQueryError(`Aggregate result missing key "${label}"`);
  }
  const entry = row[wireKey];
  if (typeof entry !== "object" || entry === null || !("value" in entry)) {
    throw new TypedQueryError(`Aggregate result "${label}" is not a { value } document`);
  }
  return (entry as { value: unknown }).value;
}
