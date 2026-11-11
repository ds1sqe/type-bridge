import { PlanCondition, PlanExpr, PlanInput, PlanValue, isClassFieldToken, valueFieldCondition, type FieldToken, type PlanOperand } from "./plan.js";

/** @internal The recording supplies implicit input binding and ownership checks. */
export interface ValueRecorder {
  bind(value: unknown, floating?: boolean): PlanInput<unknown>;
}

type Operand<T> = T | Value<T> | Expr<T> | PlanInput<T>;
type Comparable<T> = T | Value<T> | PlanInput<T>;
type Arithmetic = "add" | "sub" | "mul" | "div";

function operand<T>(recorder: ValueRecorder, value: Operand<T>): PlanExpr | PlanOperand {
  if (value instanceof Value || value instanceof Expr) return value.raw;
  return value instanceof PlanInput ? value : recorder.bind(value);
}

/** A typed scalar expression recorded through the shared core builder. */
export class Expr<T> {
  declare private readonly scalar: T;
  /** @internal */
  constructor(readonly raw: PlanExpr, private readonly recorder: ValueRecorder) { Object.freeze(this); }

  private arithmetic(op: Arithmetic, other: Operand<T>): Expr<T> {
    return new Expr(this.raw[op](operand(this.recorder, other)), this.recorder);
  }
  add(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<T> { return this.arithmetic("add", other); }
  sub(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<T> { return this.arithmetic("sub", other); }
  mul(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<T> { return this.arithmetic("mul", other); }
  div(this: T extends bigint | number ? Expr<T> : never, other: Operand<T>): Expr<number> {
    return new Expr(this.raw.div(operand(this.recorder, other)), {
      bind: (value) => this.recorder.bind(value, true),
    });
  }
}

/** A bound model field; plain operands become given inputs in its recording. */
export class Value<T> {
  declare private readonly scalar: T;
  /** @internal */
  constructor(readonly raw: PlanValue, private readonly recorder: ValueRecorder) { Object.freeze(this); }

  private comparison(op: "eq" | "ne" | "lt" | "le" | "gt" | "ge", other: Comparable<T> | FieldToken<T>): PlanCondition {
    if (isClassFieldToken(other)) return valueFieldCondition(this as Value<unknown>, op, other);
    const right = operand(this.recorder, other as Comparable<T>);
    if (right instanceof PlanExpr) throw new TypeError("a comparison requires a bound scalar");
    return this.raw[op](right);
  }
  eq(other: Comparable<T> | FieldToken<T>): PlanCondition { return this.comparison("eq", other); }
  ne(other: Comparable<T> | FieldToken<T>): PlanCondition { return this.comparison("ne", other); }
  lt(other: Comparable<T> | FieldToken<T>): PlanCondition { return this.comparison("lt", other); }
  le(other: Comparable<T> | FieldToken<T>): PlanCondition { return this.comparison("le", other); }
  gt(other: Comparable<T> | FieldToken<T>): PlanCondition { return this.comparison("gt", other); }
  ge(other: Comparable<T> | FieldToken<T>): PlanCondition { return this.comparison("ge", other); }
  private stringComparison(op: "contains" | "starts_with" | "ends_with" | "like", other: Comparable<T>): PlanCondition {
    const right = operand(this.recorder, other);
    if (right instanceof PlanExpr) throw new TypeError("a comparison requires a bound scalar");
    return new PlanCondition(this.raw, op, right);
  }
  contains(this: Value<string>, other: Comparable<string>): PlanCondition { return this.stringComparison("contains", other); }
  startsWith(this: Value<string>, other: Comparable<string>): PlanCondition { return this.stringComparison("starts_with", other); }
  endsWith(this: Value<string>, other: Comparable<string>): PlanCondition { return this.stringComparison("ends_with", other); }
  like(this: Value<string>, other: Comparable<string>): PlanCondition { return this.stringComparison("like", other); }
  private arithmetic(op: Arithmetic, other: Operand<T>): Expr<T> {
    return new Expr(this.raw[op](operand(this.recorder, other)), this.recorder);
  }
  add(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<T> { return this.arithmetic("add", other); }
  sub(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<T> { return this.arithmetic("sub", other); }
  mul(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<T> { return this.arithmetic("mul", other); }
  div(this: T extends bigint | number ? Value<T> : never, other: Operand<T>): Expr<number> {
    return new Expr(this.raw.div(operand(this.recorder, other)), {
      bind: (value) => this.recorder.bind(value, true),
    });
  }
}
