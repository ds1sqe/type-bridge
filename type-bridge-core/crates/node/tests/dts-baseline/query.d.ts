import type { AggregateInput, AttributeValue, DynamicComparisonOp, DynamicExpr, DynamicQuerySpec, DynamicSort, DynamicSortDir } from "./index.js";
/** Minimal field label consumed by the separately retained V1 group-by facade. */
export interface QueryGroupField {
    readonly attrName: string;
}
/** Raised when a Rust aggregate/group-by result does not match the documented shape. */
export declare class TypedQueryError extends Error {
    constructor(message: string);
}
/**
 * A typed query-filter expression. Subclasses serialize to the wire `DynamicExpr`
 * tree that the Rust core lowers into a database query; the boolean combinators
 * (`and_`, `or_`, `not_`) build nested expressions so a whole filter tree is one
 * value.
 */
export declare abstract class QueryExpr {
    /** Serialize to the Rust `DynamicExpr` wire shape. */
    abstract toExpr(): DynamicExpr;
    and_(...others: QueryExpr[]): QueryExpr;
    or_(...others: QueryExpr[]): QueryExpr;
    not_(): QueryExpr;
}
/**
 * Attribute comparison or string-operator leaf. String operators
 * (`starts_with`/`ends_with`) carry the raw literal — Rust owns anchoring and
 * escaping (Plan 09 Gap B).
 */
export declare class ComparisonExpr extends QueryExpr {
    private readonly attrName;
    private readonly operator;
    private readonly value;
    constructor(attrName: string, operator: DynamicComparisonOp, value: AttributeValue);
    toExpr(): DynamicExpr;
}
/** Conjunction/disjunction of child expressions. */
export declare class BooleanExpr extends QueryExpr {
    private readonly op;
    private readonly children;
    constructor(op: "and" | "or", children: QueryExpr[]);
    toExpr(): DynamicExpr;
}
/** Negation of a child expression. */
export declare class NotExpr extends QueryExpr {
    private readonly child;
    constructor(child: QueryExpr);
    toExpr(): DynamicExpr;
}
/** A typed sort key produced by `Attr.asc()` / `Attr.desc()`. */
export declare class SortExpr {
    private readonly attrName;
    private readonly direction;
    constructor(attrName: string, direction: DynamicSortDir);
    toSort(): DynamicSort;
}
/**
 * A typed aggregate produced by `Attr.avg()` / `agg.count()` etc. `input.result_key`
 * is the wire reduce variable (must be a valid identifier — attribute names with
 * `-` are sanitized to `_`); `resultKey` is the user-facing key in the normalized
 * result, which preserves the original attribute name. The wire `function` is the
 * TypeDB reduce name (`mean` for `avg`).
 */
export declare class AggregateSpec {
    readonly input: AggregateInput;
    readonly resultKey: string;
    constructor(input: AggregateInput, resultKey: string);
}
/** Field-independent aggregate helpers. */
export declare const agg: {
    /** Count matching rows. Normalizes to the `count` result key. */
    readonly count: () => AggregateSpec;
};
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
export declare class TypedQuery<T, Row> {
    #private;
    constructor(manager: DynamicManagerLike<Row>, hydrate: (rows: Row[]) => T[]);
    /** Add filter expressions. Multiple calls (and multiple args) are ANDed in Rust. */
    filter(...exprs: QueryExpr[]): this;
    orderBy(...sorts: SortExpr[]): this;
    limit(limit: number): this;
    offset(offset: number): this;
    all(): T[];
    execute(): T[];
    first(): T | null;
    count(): bigint;
    exists(): boolean;
    /** Reduce matching rows to one normalized result object keyed by aggregate result keys. */
    aggregate(...aggregates: AggregateSpec[]): Record<string, unknown>;
    groupBy(...attrs: QueryGroupField[]): TypedGroupByQuery<Row>;
}
/**
 * Grouped-aggregate query produced by `TypedQuery.groupBy(...)`. `aggregate(...)`
 * delegates to the Rust group-by seam and normalizes each group row into an
 * object keyed by group-attribute names plus aggregate result keys.
 */
export declare class TypedGroupByQuery<Row> {
    #private;
    constructor(manager: DynamicManagerLike<Row>, exprs: DynamicExpr[], groupAttrs: QueryGroupField[]);
    aggregate(...aggregates: AggregateSpec[]): Record<string, unknown>[];
}
export {};
