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
export type PlanOperand = PlanValue | PlanInput | PlanLiteral;
type Comparator = "equal" | "not_equal" | "less" | "less_or_equal" | "greater" | "greater_or_equal";
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
export declare class PlanInputValue {
    readonly [planHandleKind]: "input-value";
    /** @internal */
    constructor(input: PlanInput, value: PlanLiteral | null);
}
/**
 * A typed input column; rows supply its values through
 * {@link PlanInput.value}. Many rows run the one plan over all of them.
 */
export declare class PlanInput {
    /** @internal */
    constructor();
    /** This column's value in one input row, of the column's value type. */
    value(value: string | bigint | number | boolean | AttributeValue): PlanInputValue;
    /** An absent value for an optional column. */
    absent(): PlanInputValue;
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
    constructor();
    /** Match things of exactly `model`. */
    exact<Id extends string>(model: PlanModelToken<Id>): PlanThing<Id>;
    /** Match things of `model` or any of its subtypes. */
    subtypes<Id extends string>(model: PlanModelToken<Id>): PlanThing<Id>;
    /** Bind the value of `thing`'s field. Things without the field do not match. */
    field<Id extends string>(thing: PlanThing<Id>, token: PlanFieldToken<Owned<Id>>): PlanValue;
    /** Declare an input column named `name`; each {@link row} supplies a value. */
    input(name: string, valueType: PlanValueType): PlanInput;
    /** Declare an optional input column; rows may give it {@link PlanInput.absent}. */
    optionalInput(name: string, valueType: PlanValueType): PlanInput;
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
/** @internal The native program behind `plan`, for staging. */
export declare function nativePlan(plan: Plan): NativePlan;
/**
 * A validated `query-plan/v3` program. Stage it on a
 * {@link StagedTransaction} from `RustDatabase.stagedTransaction()` or
 * `ProxyConnection.transaction()`.
 */
export declare class Plan {
    /** @internal */
    constructor(native: NativePlan);
    /** Start an empty program. */
    static builder(): PlanBuilder;
    /** The plan's canonical `query-plan/v3` bytes, identical in every SDK. */
    canonicalBytes(): Uint8Array;
    /** The plan fingerprint digest, hex encoded. */
    fingerprint(): string;
    /** Whether the plan writes, which makes an inferred transaction kind `write`. */
    writes(): boolean;
}
export {};
