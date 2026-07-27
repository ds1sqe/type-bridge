/** Explicit immutable budgets bound into every remote model-query request. */
export interface RemoteQueryLimits {
  readonly maxItems: bigint;
  readonly maxBytes: bigint;
  readonly maxCollectionMembers: bigint;
  readonly maxGraphNodes: bigint;
  readonly maxAttributeValues: bigint;
  readonly maxRolePlayers: bigint;
  readonly deadlineMs?: bigint | null;
}
