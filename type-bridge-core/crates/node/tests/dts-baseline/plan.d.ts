/**
 * Typed `query-plan/v3` programs built from generated model tokens.
 *
 * A {@link PlanBuilder} declares things, field values, inputs and `let`
 * values, then the writes over them. {@link PlanBuilder.build} validates the
 * program against a schema authority and returns a {@link Plan}, which a
 * {@link StagedTransaction} runs at commit. The program is one pipeline: a
 * write plan that matches nothing writes nothing.
 *
 * Every call forwards to the one Rust builder the Rust, Python and C SDKs
 * use, so the same program gives the same canonical bytes in every
 * language:
 *
 * - Handles are numbered in creation order.
 * - `exact`, `subtypes`, `field`, `links`, `where` and `let` add to the
 *   plan's single leading match, in call order.
 * - Writes run in call order after the match. Consecutive writes of one
 *   kind (`update`, `insert`, `put`, `delete`) share one stage.
 * - Things and fields are named from their labels (`item`, `item_stock`),
 *   with `_2`, `_3`, ... on collision. `input` and `let` names must be
 *   fresh.
 *
 * Content errors (an invalid literal, a duplicate name, a field the thing
 * does not own) never throw from a builder call: the first one is thrown by
 * `build` as a `QueryV2Error`. Handles belong to the builder that created
 * them; passing one to another builder throws a `TypeError` at once.
 *
 * @module
 */
import type { AttributeValue, QueryV2Authority } from "./index.js";
import type { NativePlan } from "./native.js";
import type { PlanAnswer } from "./staged-transaction.js";
import { Value } from "./plan-values.js";
import type { QueryLimits, InstalledRuntimeProjection, NativeAuthoredPlanResult } from "./runtime-projection.js";
/** Query policy chosen once when a recording is created. */
export interface PlanOptions {
    readonly queryLimits?: QueryLimits;
}
/** A generated model token, such as `Item`: its canonical type identity. */
export interface PlanModelToken<Id extends string = string> {
    readonly typeKey: Id;
}
/** A generated field token, such as `Item.fields.stock`. */
export interface PlanFieldToken<Owner extends string = string> {
    readonly kind: "field";
    readonly owner: Owner;
    readonly metadata: unknown;
}
/** A generated role token, such as `Purchase.roles.buyer`. */
export interface PlanRoleToken<Owner extends string = string> {
    readonly kind: "role";
    readonly owner: Owner;
    readonly role: string;
}
/** The scalar domain of an input column. */
export type PlanValueType = "string" | "long" | "double" | "boolean" | "date" | "datetime" | "datetime_tz" | "decimal" | "duration";
/**
 * A literal: a `string`, a `bigint` (long) or a `boolean`, or a typed
 * value from `long()`, `double()`, `decimal()`, `date()` and the other
 * value helpers. Plain numbers are not literals; use `double()` or a
 * `bigint`.
 */
export type PlanLiteral = string | bigint | boolean | AttributeValue;
/** A scalar operand: a {@link PlanValue}, a {@link PlanInput}, or a literal. */
export type PlanOperand = PlanValue | PlanInput<unknown> | PlanLiteral;
type PlanHostTypes = {
    string: string;
    long: bigint;
    double: number;
    boolean: boolean;
    date: Date;
    datetime: Date;
    datetime_tz: Date;
    decimal: string;
    duration: string;
};
type LegacyInputValue = string | bigint | number | boolean | AttributeValue;
type Comparator = "equal" | "not_equal" | "less" | "less_or_equal" | "greater" | "greater_or_equal" | "contains" | "starts_with" | "ends_with" | "like";
declare const planThingModel: unique symbol;
declare const planHandleKind: unique symbol;
type Owned<Id> = [Id][Id extends unknown ? 0 : never];
/**
 * A value expression for {@link PlanBuilder.let}: arithmetic over values,
 * inputs and literals (`stock.sub(1n)`), or a builtin (`PlanExpr.abs(x)`).
 */
export declare class PlanExpr {
    /** @internal */
    constructor();
    /** `this + other`. */
    add(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this - other`. */
    sub(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this * other`. */
    mul(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this / other`, floating division. */
    div(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this % other`. */
    mod(other: PlanExpr | PlanOperand): PlanExpr;
    /** `base` to the power `exponent`. */
    static pow(base: PlanExpr | PlanOperand, exponent: PlanExpr | PlanOperand): PlanExpr;
    /** Absolute value. */
    static abs(value: PlanExpr | PlanOperand): PlanExpr;
    /** Smallest integer not below a decimal or double. */
    static ceil(value: PlanExpr | PlanOperand): PlanExpr;
    /** Largest integer not above a decimal or double. */
    static floor(value: PlanExpr | PlanOperand): PlanExpr;
    /** Nearest integer to a decimal or double. */
    static round(value: PlanExpr | PlanOperand): PlanExpr;
    /** The smaller of two values of one numeric domain. */
    static min(left: PlanExpr | PlanOperand, right: PlanExpr | PlanOperand): PlanExpr;
    /** The larger of two values of one numeric domain. */
    static max(left: PlanExpr | PlanOperand, right: PlanExpr | PlanOperand): PlanExpr;
    /** The length of a string. */
    static length(value: PlanExpr | PlanOperand): PlanExpr;
}
/** A comparison between two operands, added with {@link PlanBuilder.where}. */
export declare class PlanCondition {
    readonly [planHandleKind]: "condition";
    /** @internal */
    constructor(left: PlanOperand, comparator: Comparator, right: PlanOperand);
}
/** A matched or created thing of the model with type identity `Id`. */
export declare class PlanThing<Id extends string = string> {
    readonly [planThingModel]: Id;
    /** @internal */
    constructor();
}
/** A scalar value bound by {@link PlanBuilder.field} or {@link PlanBuilder.let}. */
export declare class PlanValue {
    /** @internal */
    constructor();
    /** Require this value to equal `other`. */
    eq(other: PlanOperand): PlanCondition;
    /** Require this value to differ from `other`. */
    ne(other: PlanOperand): PlanCondition;
    /** Require this value to be less than `other`. */
    lt(other: PlanOperand): PlanCondition;
    /** Require this value to be at most `other`. */
    le(other: PlanOperand): PlanCondition;
    /** Require this value to be greater than `other`. */
    gt(other: PlanOperand): PlanCondition;
    /** Require this value to be at least `other`. */
    ge(other: PlanOperand): PlanCondition;
    /** `this + other`. */
    add(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this - other`. */
    sub(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this * other`. */
    mul(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this / other`, floating division. */
    div(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this % other`. */
    mod(other: PlanExpr | PlanOperand): PlanExpr;
}
/** One column's value in an input row; see {@link PlanBuilder.row}. */
export declare class PlanInputValue<T = unknown> {
    readonly [planHandleKind]: "input-value";
    /** @internal */
    constructor(input: PlanInput<T>, value: T | null);
}
/**
 * A typed input column; rows supply its values through
 * {@link PlanInput.value}. Many rows run the one plan over all of them.
 */
export declare class PlanInput<T = LegacyInputValue> {
    private readonly scalar;
    /** @internal */
    constructor();
    /** This column's value in one input row, of the column's value type. */
    value(value: T): PlanInputValue<T>;
    /** An absent value for an optional column. */
    absent(): PlanInputValue<T>;
    /** `this + other`. */
    add(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this - other`. */
    sub(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this * other`. */
    mul(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this / other`, floating division. */
    div(other: PlanExpr | PlanOperand): PlanExpr;
    /** `this % other`. */
    mod(other: PlanExpr | PlanOperand): PlanExpr;
}
/**
 * A thing being inserted or put; add its fields and players, then take its
 * handle with {@link PlanNewThing.thing}.
 */
export declare class PlanNewThing<Id extends string = string> {
    #private;
    /** @internal */
    constructor(thing: PlanThing<Id>);
    /** Give the new thing a field value. */
    field(token: PlanFieldToken<Owned<Id>>, value: PlanOperand): this;
    /** Give the new relation a role player. */
    player(role: PlanRoleToken<Owned<Id>>, player: PlanThing): this;
    /** The new thing's handle. */
    thing(): PlanThing<Id>;
}
/**
 * Builds one {@link Plan}. See the {@link "plan" | module documentation} for
 * the rules.
 */
export declare class PlanBuilder {
    #private;
    constructor(options?: PlanOptions);
    /** Match things of exactly `model`. */
    exact<Id extends string>(model: PlanModelToken<Id>): PlanThing<Id>;
    /** Match things of `model` or any of its subtypes. */
    subtypes<Id extends string>(model: PlanModelToken<Id>): PlanThing<Id>;
    /** Bind the value of `thing`'s field. Things without the field do not match. */
    field<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>): PlanValue;
    /** @internal Generated field access uses the shared binding cache. */
    fieldValue(thing: PlanThing, token: PlanFieldToken): PlanValue;
    /** @internal Generated IID lookup. */
    byIid(thing: PlanThing, iid: string): void;
    /** @internal Add a player without replacing existing role players. */
    link(relation: PlanThing, role: PlanRoleToken, player: PlanThing): void;
    /** @internal Lift a generated helper's caller value into given. */
    bindValue(thing: PlanThing, field: string, kind: PlanValueType, value: unknown): PlanInput;
    /** @internal Name a generated assignment through the shared core. */
    letValue(thing: PlanThing, field: string, value: PlanExpr): PlanValue;
    /** Declare an input column named `name`; each {@link row} supplies a value. */
    input(name: string, valueType: PlanValueType): PlanInput;
    /** Declare an optional input column; rows may give it {@link PlanInput.absent}. */
    optionalInput(name: string, valueType: PlanValueType): PlanInput;
    /** @internal Strong input typing for explicit generated templates. */
    typedInput<K extends PlanValueType>(name: string, kind: K, optional: boolean): PlanInput<PlanHostTypes[K]>;
    /** Require `condition` of every matched row. */
    where(condition: PlanCondition): this;
    /** Require `relation` to link `player` in `role`. */
    links<Id extends string>(relation: PlanThing<Id>, role: PlanRoleToken<Owned<Id>>, player: PlanThing): this;
    /** Bind `name` to `value`, evaluated for each matched row. */
    let(name: string, value: PlanExpr | PlanOperand): PlanValue;
    /** Replace `thing`'s field value with `value`. */
    update<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>, value: PlanOperand): this;
    /** Make `player` the `role` player of `relation`. */
    updatePlayer<Id extends string>(relation: PlanThing<Id>, role: PlanRoleToken<Owned<Id>>, player: PlanThing): this;
    /** Create a thing of `model`; give it fields and players on the result. */
    insert<Id extends string>(model: PlanModelToken<Id>): PlanNewThing<Id>;
    /**
     * Match a thing of `model` with the fields and players given on the
     * result, or create it.
     */
    put<Id extends string>(model: PlanModelToken<Id>): PlanNewThing<Id>;
    /** Delete `thing`. */
    delete(thing: PlanThing): this;
    /** Delete `thing`'s field, keeping the thing. */
    deleteField<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>): this;
    /** Remove `player` from `relation`'s `role`. */
    deletePlayer<Id extends string>(relation: PlanThing<Id>, role: PlanRoleToken<Owned<Id>>, player: PlanThing): this;
    /**
     * Add one input row. Every column appears at most once; a missing
     * optional column is absent.
     */
    row(values: readonly PlanInputValue[]): this;
    /**
     * Project `columns`, in order. A write plan then answers these rows from
     * the same pipeline instead of its matched count.
     */
    returning(columns: readonly (PlanThing | PlanValue)[]): this;
    /** Describe `thing` and its listed players; bare roles are required. */
    fetch<Id extends string>(thing: PlanThing<Id>, roles?: ReadonlyArray<PlanRoleToken<Owned<Id>> | readonly [PlanRoleToken<Owned<Id>>, boolean]>): this;
    /** Answer the number of rows a read plan matches. */
    count(): this;
    /** Answer whether a read plan matches any row. */
    exists(): this;
    /**
     * Validate the program against `authority`, a generated package's
     * `schemaAuthority()` or a `QueryV2Authority` over declared-schema bytes.
     *
     * A plan with writes answers its matched row count, or the
     * {@link returning} rows. A read answers rows, or {@link count} /
     * {@link exists}. Throws a `QueryV2Error` for the first invalid call, a
     * write plan that also asked for count or exists, or a plan the schema
     * rejects (for example a field the thing does not own, or a key update).
     */
    build(authority: QueryV2Authority): Plan;
}
type ScalarEncoder = (kind: PlanValueType, value: unknown) => unknown;
/** @internal The native program behind `plan`, for staging. */
export declare function nativePlan(plan: Plan<unknown>): NativePlan;
/** @internal Apply the typed projection chosen by a generated helper. */
export declare function decodePlan<R>(plan: Plan<R>, answerJson: string, projection?: NativeAuthoredPlanResult): R;
/**
 * A validated `query-plan/v3` program. Stage it on a
 * {@link StagedTransaction} from `RustDatabase.stagedTransaction()` or
 * `ProxyConnection.transaction()`.
 */
export interface PlanExplanation {
    /** Canonical plan-shape fingerprint; excludes transported caller values. */
    readonly fingerprint: {
        readonly algorithm: string;
        readonly canonicalization: string;
        readonly domain: string;
        readonly digest: string;
    };
    readonly format: string;
    readonly required_capabilities: readonly string[];
    readonly operation: {
        readonly kind: "rows" | "count" | "exists" | "write";
        readonly returns?: "rows" | "matched";
    };
    readonly bindings: readonly {
        readonly id: number;
        readonly variable: string;
        readonly possible_types: readonly {
            readonly kind: "entity" | "relation" | "attribute";
            readonly label: string;
        }[];
    }[];
    readonly inputs: readonly {
        readonly public_name: string;
        readonly value_type: PlanValueType;
        readonly optional: boolean;
    }[];
    readonly given: {
        readonly row_count: number;
        readonly canonical_bytes: number;
    };
    readonly statements: readonly string[];
    readonly read_statements: number;
    readonly write_statements: number;
}
export declare class Plan<R = PlanAnswer> {
    private readonly resultType;
    /** @internal */
    constructor(native?: NativePlan, decoder?: (answerJson: string) => R);
    /** Start an empty program. */
    static builder(): PlanBuilder;
    /** Record generated helpers synchronously; awaited continuations need a new context. */
    static build<R>(body: (plan: PlanRecording) => Plan<R>, options?: PlanOptions): Plan<R>;
    static build(body: (plan: PlanRecording) => void, options?: PlanOptions): Plan<Matched>;
    /** Declare reusable explicit input columns before recording a template. */
    static template(options?: PlanOptions): PlanTemplate;
    /** The plan's canonical `query-plan/v3` bytes, identical in every SDK. */
    canonicalBytes(): Uint8Array;
    /** Exact executed statements. Given values are omitted; embedded plan constants remain. */
    explain(): PlanExplanation;
    /** The plan fingerprint digest, hex encoded. */
    fingerprint(): string;
    /** Whether the plan writes, which makes an inferred transaction kind `write`. */
    writes(): boolean;
    /** Bind explicit template columns to a fresh immutable invocation. */
    bind(rows: Iterable<Iterable<PlanInputValue>>): Plan<R>;
}
/** The count produced by an unprojected write. */
export declare class Matched {
    readonly count: bigint;
    constructor(count: bigint);
}
type ResultSelection = {
    readonly recording: PlanRecording;
    readonly kind: "matched" | "count" | "exists" | "models" | "rows";
    readonly thing?: number;
    readonly declared?: string;
    readonly roles?: readonly {
        role: string;
        required: boolean;
    }[];
    readonly decode?: (plan: NativePlan, answerJson: string, projection?: NativeAuthoredPlanResult) => unknown;
    readonly columns?: readonly (PlanValue | PlanThing)[];
    readonly modelSlots?: readonly {
        readonly binding: number;
        readonly declared: string;
    }[];
};
/** A reusable plan with explicit input columns and rows. */
export declare class PlanTemplate {
    #private;
    constructor(options?: PlanOptions);
    private requireOpen;
    input<K extends PlanValueType>(name: string, kind: K): PlanInput<PlanHostTypes[K]>;
    optionalInput<K extends PlanValueType>(name: string, kind: K): PlanInput<PlanHostTypes[K]>;
    row(cells: readonly PlanInputValue[]): this;
    build<R>(body: (plan: PlanRecording) => Plan<R>): Plan<R>;
}
/** The synchronous recording supplied to Plan.build. */
export declare class PlanRecording {
    #private;
    /** @internal */
    readonly builder: PlanBuilder;
    constructor(options?: PlanOptions);
    /** @internal */
    static current(): PlanRecording;
    /** @internal One generated package supplies the authority for a recording. */
    use(authority: QueryV2Authority, decodeScalars?: (plan: NativePlan, answerJson: string) => unknown[][], encodeScalar?: ScalarEncoder, projection?: InstalledRuntimeProjection, decodeRows?: (projection: NativeAuthoredPlanResult) => unknown[][]): void;
    /** @internal */
    result<R>(selection: Omit<ResultSelection, "recording">): Plan<R>;
    /** Answer the matched row count. */
    count(): Plan<bigint>;
    /** Answer whether any row matches. */
    exists(): Plan<boolean>;
    /** Require conditions involving bound values from this recording. */
    require(...conditions: PlanCondition[]): void;
    /** Select model and scalar tuples in argument order. */
    returning<const Values extends readonly (Value<unknown> | Handle<unknown>)[]>(...values: Values): Plan<{
        -readonly [K in keyof Values]: Values[K] extends Value<infer T> ? T : Values[K] extends Handle<infer M> ? M : never;
    }[]>;
    /** @internal Defer rows until the generated package supplies its host scalar codec. */
    row(cells: readonly PlanInputValue[]): void;
    /** @internal */
    finish<R>(selected: Plan<R> | void): Plan<R> | Plan<Matched>;
}
/** @internal Generated model metadata and its existing scalar codec. */
export interface PlanModelAuthoring<M> {
    readonly model: PlanModelToken;
    readonly projection: () => InstalledRuntimeProjection;
    readonly decodeRows: (projection: NativeAuthoredPlanResult) => unknown[][];
    readonly authority: () => QueryV2Authority;
    readonly isFieldToken: (token: object) => boolean;
    readonly fields: Readonly<Record<string, {
        readonly token: PlanFieldToken;
        readonly kind: PlanValueType;
        readonly site: string;
        readonly encode: (value: unknown) => unknown;
    }>>;
    readonly roles: readonly {
        readonly name: string;
        readonly token: PlanRoleToken;
        readonly role: string;
        readonly required: boolean;
    }[];
    readonly decode: (plan: NativePlan, answerJson: string, projection?: NativeAuthoredPlanResult) => M[];
    readonly decodeScalars: (plan: NativePlan, answerJson: string) => unknown[][];
    readonly encodeScalar: ScalarEncoder;
}
/** A model bound inside a synchronous plan recording. */
export declare class Handle<M> {
    #private;
    readonly recording: PlanRecording;
    readonly thing: PlanThing;
    readonly definition: PlanModelAuthoring<M>;
    private readonly modelType;
    /** @internal */
    constructor(recording: PlanRecording, thing: PlanThing, definition: PlanModelAuthoring<M>);
    private open;
    /** @internal Generated properties supply their schema field name and scalar type. */
    field<T>(name: string): Value<T>;
    where(...conditions: PlanCondition[]): this;
    /** @internal Per-model set signatures restrict the available fields. */
    setFields(fields: Readonly<Record<string, unknown>>): Plan<M[]>;
    unset(field: PlanFieldToken): Plan<M[]>;
    /** @internal Generated role signatures supply nominal player handles. */
    linkPlayers(players: Readonly<Record<string, Handle<unknown> | readonly Handle<unknown>[] | undefined>>, remove?: boolean): Plan<M[]>;
    delete(): Plan<Matched>;
    deleteReturning(): Plan<M[]>;
    /** @internal */
    models(): Plan<M[]>;
}
/** @internal Shared recorder behind generated match/subtypes/byIid helpers. */
export declare function planMatch<M, H extends Handle<M>>(definition: PlanModelAuthoring<M>, HandleClass: new (recording: PlanRecording, thing: PlanThing, definition: PlanModelAuthoring<M>) => H, fields?: Readonly<Record<string, unknown>>, options?: {
    subtypes?: boolean;
    iid?: string;
    key?: readonly [string, unknown];
}): H;
/** @internal Shared scalar insert/put recorder for generated factories. */
export declare function planInsert<M>(definition: PlanModelAuthoring<M>, fields: Readonly<Record<string, unknown>>, players?: Readonly<Record<string, Handle<unknown> | readonly Handle<unknown>[] | undefined>>, put?: boolean): Plan<M[]>;
/** @internal Core groups, splits and correlates generated entity Values rows. */
export declare function planInsertMany<M>(definition: PlanModelAuthoring<M>, rows: Iterable<object>, put?: boolean): Plan<M[]>;
/** A generated class field, resolved against the handle passed to where(). */
export interface FieldToken<T> extends PlanFieldToken {
    eq(other: T | Value<T> | PlanInput<T>): PlanCondition;
    ne(other: T | Value<T> | PlanInput<T>): PlanCondition;
    lt(other: T | Value<T> | PlanInput<T>): PlanCondition;
    le(other: T | Value<T> | PlanInput<T>): PlanCondition;
    gt(other: T | Value<T> | PlanInput<T>): PlanCondition;
    ge(other: T | Value<T> | PlanInput<T>): PlanCondition;
    contains(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
    startsWith(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
    endsWith(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
    like(this: FieldToken<string>, other: string | Value<string> | PlanInput<string>): PlanCondition;
}
/** @internal Decorate the generated runtime's existing nominal field token. */
export declare function planFieldToken<T, Token extends PlanFieldToken>(token: Token): Token & FieldToken<T>;
export {};
