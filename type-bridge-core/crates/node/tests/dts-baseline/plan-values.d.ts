import { PlanCondition, PlanExpr, PlanInput, PlanValue, type FieldToken } from "./plan.js";
/** @internal The recording supplies implicit input binding and ownership checks. */
export interface ValueRecorder {
    bind(value: unknown, floating?: boolean): PlanInput<unknown>;
}
type Operand<T> = T | Value<T> | Expr<T> | PlanInput<T>;
type Comparable<T> = T | Value<T> | PlanInput<T>;
/** A typed scalar expression recorded through the shared core builder. */
export declare class Expr<T> {
    readonly raw: PlanExpr;
    private readonly recorder;
    private readonly scalar;
    /** @internal */
    constructor(raw: PlanExpr, recorder: ValueRecorder);
    private arithmetic;
    add(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<T>;
    sub(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<T>;
    mul(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<T>;
    div(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<number>;
}
/** A bound model field; plain operands become given inputs in its recording. */
export declare class Value<T> {
    readonly raw: PlanValue;
    private readonly recorder;
    private readonly scalar;
    /** @internal */
    constructor(raw: PlanValue, recorder: ValueRecorder);
    private comparison;
    eq(other: Comparable<T> | FieldToken<T>): PlanCondition;
    ne(other: Comparable<T> | FieldToken<T>): PlanCondition;
    lt(other: Comparable<T> | FieldToken<T>): PlanCondition;
    le(other: Comparable<T> | FieldToken<T>): PlanCondition;
    gt(other: Comparable<T> | FieldToken<T>): PlanCondition;
    ge(other: Comparable<T> | FieldToken<T>): PlanCondition;
    private stringComparison;
    contains(this: Value<string>, other: Comparable<string>): PlanCondition;
    startsWith(this: Value<string>, other: Comparable<string>): PlanCondition;
    endsWith(this: Value<string>, other: Comparable<string>): PlanCondition;
    like(this: Value<string>, other: Comparable<string>): PlanCondition;
    private arithmetic;
    add(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<T>;
    sub(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<T>;
    mul(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<T>;
    div(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<number>;
}
export {};
