import { type DynamicEntityRow, type DynamicRelationRow, type EntityDescriptor, type RelationDescriptor, type RolePlayerInput, type RustDatabase, type RustTransactionContext } from "./index.js";
import { TypedQuery } from "./query.js";
import { type Attribute } from "./attribute.js";
import { type IidBearing } from "./iid.js";
import type { EntitySchema, RelationSchema } from "./model.js";
export type ManagerConnection = RustDatabase | RustTransactionContext;
export type ExactFilters<T> = Partial<{
    readonly [Key in keyof T as NonNullable<T[Key]> extends Attribute<unknown, string> ? Key : never]: NonNullable<T[Key]>;
}>;
type EntityConstructor<T extends IidBearing> = (new (values: Record<string, unknown>) => T) & {
    readonly schema: EntitySchema;
    descriptor(): EntityDescriptor;
};
type RelationConstructor<T extends IidBearing> = (new (values: Record<string, unknown>) => T) & {
    readonly schema: RelationSchema;
    descriptor(): RelationDescriptor;
};
export declare class TypedEntityManager<T extends IidBearing> {
    #private;
    constructor(modelClass: EntityConstructor<T>, connection: ManagerConnection);
    insert(instance: T): T;
    insertMany(instances: readonly T[]): T[];
    put(instance: T): T;
    putMany(instances: readonly T[]): T[];
    update(instance: T): T;
    get(filters?: ExactFilters<T> | null): T[];
    all(): T[];
    getByIid(iid: string): T | null;
    count(filters?: ExactFilters<T> | null): bigint;
    query(): TypedQuery<T, DynamicEntityRow>;
    delete(instanceOrIid: T | string): void;
}
export declare class TypedRelationManager<T extends IidBearing> {
    #private;
    constructor(modelClass: RelationConstructor<T>, connection: ManagerConnection);
    insert(instance: T): T;
    insertMany(instances: readonly T[]): T[];
    put(instance: T): T;
    putMany(instances: readonly T[]): T[];
    update(instance: T): T;
    get(filters?: ExactFilters<T> | null): T[];
    all(): T[];
    getByIid(iid: string): T | null;
    count(filters?: ExactFilters<T> | null): bigint;
    query(): TypedQuery<T, DynamicRelationRow>;
    delete(instanceOrIid: T | string): void;
}
export declare function entityManagerFor<T extends IidBearing>(modelClass: EntityConstructor<T>, connection: ManagerConnection): TypedEntityManager<T>;
export declare function relationManagerFor<T extends IidBearing>(modelClass: RelationConstructor<T>, connection: ManagerConnection): TypedRelationManager<T>;
export declare function buildRolePlayers(instance: IidBearing, schema: RelationSchema): RolePlayerInput[];
export {};
