export interface Cardinality {
    min: number;
    max: number | null;
}
export interface OwnedAttribute {
    name: string;
    is_key: boolean;
    is_unique: boolean;
    is_cascade: boolean;
    subkey_group: string | null;
    cardinality: Cardinality | null;
}
export interface PlayedRole {
    role_ref: string;
    cardinality: Cardinality | null;
}
export interface RoleSpec {
    name: string;
    overrides: string | null;
    cardinality: Cardinality | null;
    distinct: boolean;
    ordered: boolean;
}
export interface EntityType {
    name: string;
    parent: string | null;
    is_abstract: boolean;
    owns: OwnedAttribute[];
    owns_order: string[];
    plays: PlayedRole[];
}
export interface RelationType {
    name: string;
    parent: string | null;
    is_abstract: boolean;
    roles: RoleSpec[];
    owns: OwnedAttribute[];
    owns_order: string[];
    plays: PlayedRole[];
}
export interface AttributeType {
    name: string;
    value_type: string;
    parent: string | null;
    is_abstract: boolean;
    is_independent: boolean;
    regex: string | null;
    allowed_values: string[] | null;
    range_min: string | null;
    range_max: string | null;
}
export interface Parameter {
    name: string;
    type: string;
}
export interface ReturnTypeItem {
    name: string;
    optional: boolean;
}
export interface ReturnType {
    is_stream: boolean;
    types: ReturnTypeItem[];
}
export interface FunctionType {
    name: string;
    parameters: Parameter[];
    return_type: ReturnType;
}
export interface StructField {
    name: string;
    value_type: string;
    optional: boolean;
}
export interface StructType {
    name: string;
    fields: StructField[];
}
/** The complete parsed and inheritance-resolved TypeDB schema. */
export interface TypeSchema {
    entities: Record<string, EntityType>;
    relations: Record<string, RelationType>;
    attributes: Record<string, AttributeType>;
    functions: Record<string, FunctionType>;
    structs: Record<string, StructType>;
}
/**
 * The slice of the native module the parser needs. The full native module
 * (the `loadNative()` result from the package root) satisfies this; callers
 * inject it, matching the runtime convention used everywhere else in the
 * surface (e.g. `Model.manager(db)`). Injection keeps this module free of any
 * native-resolution path, so it survives the package layout migration intact.
 */
export interface SchemaParserNative {
    parseSchemaJson(input: string): string;
}
/**
 * Parse a TQL `define` block and return the fully-resolved {@link TypeSchema}.
 *
 * Marshalling-only: parsing and inheritance resolution happen in the Rust core
 * behind `parseSchemaJson`; this deserializes the JSON result. Throws on parse
 * or serialization errors (propagated from the Rust core as native errors).
 */
export declare function parseSchema(tql: string, native: SchemaParserNative): TypeSchema;
