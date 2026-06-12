/**
 * Render the `entities.ts` module text from a parsed TypeSchema.
 *
 * Mirrors `type_bridge/generator/render/entities.py` → `render_entities()`.
 * Field KEY decision logic is ported directly from `_render_attr_field`:
 *
 *   1. is_key (schema @key OR implicit key)  → field(Attr, Key)
 *   2. is_unique (schema @unique)             → field(Attr, Unique)
 *   3. cardinality === null || is_optional_single (min=0, max=1)
 *                                             → field(Attr).optional()
 *   4. is_multi (max === null || max > 1)     → field(Attr).list(Card(min, max))
 *   5. is_required && is_single (min>=1, max=1) → field(Attr)  (plain required)
 *   6. fallback                               → field(Attr).optional()
 *
 * Field keys = toFieldName(attr_name) — prefix kept, no pluralization.
 *
 * Build-time code generation; no runtime ORM logic.
 */

import type { TypeSchema, OwnedAttribute } from "../parser.js";
import { toClassName, toFieldName, isKeyAttribute, type NamingOptions } from "./naming.js";
import { isCardSingle, isCardRequired, isCardMulti, isCardOptionalSingle } from "./cardinality.js";

// ---------------------------------------------------------------------------
// Single-field renderer
// ---------------------------------------------------------------------------

/**
 * Render the `field(...)` expression for one owned attribute.
 *
 * Decision logic mirrors Python `_render_attr_field` (entities.py:38-65):
 *   Key > Unique > ordered (list) > optional_single > multi (list) > required_single > fallback optional
 *
 * Ordered list attributes use `.ordered()` / `.ordered().distinct()` instead of
 * `.list(Card(...))`.  These correspond to `owns attr[] @distinct` in TypeQL.
 */
function renderFieldExpr(
  owned: OwnedAttribute,
  attrClassName: string,
  isKey: boolean,
): string {
  const card = owned.cardinality;

  if (isKey) {
    return `field(${attrClassName}, Key)`;
  }

  if (owned.is_unique) {
    return `field(${attrClassName}, Unique)`;
  }

  // Ordered list attribute: field(Attr).ordered() or field(Attr).ordered().distinct()
  if (owned.ordered) {
    const base = `field(${attrClassName}).ordered()`;
    return owned.distinct ? `${base}.distinct()` : base;
  }

  // cardinality === null means default (optional single, same as @card(0..1))
  if (card === null || isCardOptionalSingle(card)) {
    return `field(${attrClassName}).optional()`;
  }

  if (isCardMulti(card)) {
    const maxStr = card.max === null ? "null" : String(card.max);
    return `field(${attrClassName}).list(Card(${card.min}, ${maxStr}))`;
  }

  if (isCardRequired(card) && isCardSingle(card)) {
    // required single: plain field, no modifier
    return `field(${attrClassName})`;
  }

  // Fallback: optional
  return `field(${attrClassName}).optional()`;
}

// ---------------------------------------------------------------------------
// Topological sort — parents before children (mirror `_topological_sort_entities`)
// ---------------------------------------------------------------------------

function topologicalSortEntities(schema: TypeSchema): string[] {
  const entities = schema.entities;
  const names = Object.keys(entities);
  const sorted: string[] = [];
  const visited = new Set<string>();

  function visit(name: string): void {
    if (visited.has(name)) return;
    visited.add(name);
    const parent = entities[name]?.parent;
    if (parent && entities[parent]) {
      visit(parent);
    }
    sorted.push(name);
  }

  for (const name of names) {
    visit(name);
  }
  return sorted;
}

// ---------------------------------------------------------------------------
// Main renderer
// ---------------------------------------------------------------------------

/**
 * Render the complete `entities.ts` source text from the parsed schema.
 *
 * Each entity emits:
 *   export class <ClassName> extends Entity("<schema-name>", { ... }) {}
 * or for abstract entities:
 *   export class <ClassName> extends Entity(TypeFlags({ name: "<name>", abstract: true }), { ... }) {}
 * and with parent:
 *   ... Entity("<name>", { ... }, { parent: <ParentClass> }) {}
 *
 * Field keys = toFieldName(attr_name); values = the field() expression above.
 * Attribute imports come from "./attributes"; entity parent imports also from
 * the same file.
 *
 * The import header always uses the PACKAGE ENTRYPOINT `@type-bridge/node`,
 * never a hardcoded relative path, so generated files stay valid across
 * the packaging layout change.
 */
export function renderEntities(schema: TypeSchema, options?: NamingOptions): string {
  const sortedNames = topologicalSortEntities(schema);

  // Collect which attribute class names are actually referenced (for the import)
  const referencedAttrClasses = new Set<string>();

  // Pre-compute entity class name map
  const entityClassMap = new Map<string, string>();
  for (const name of Object.keys(schema.entities)) {
    entityClassMap.set(name, toClassName(name));
  }

  // Pre-compute attribute class name map
  const attrClassMap = new Map<string, string>();
  for (const name of Object.keys(schema.attributes)) {
    attrClassMap.set(name, toClassName(name));
  }

  // Collect imports by scanning all entities
  for (const entityName of sortedNames) {
    const entity = schema.entities[entityName];
    if (!entity) continue;

    // Determine which attrs this entity owns LOCALLY (not inherited from parent)
    const parentOwns = new Set<string>();
    if (entity.parent) {
      const parentEntity = schema.entities[entity.parent];
      if (parentEntity) {
        for (const o of parentEntity.owns) {
          parentOwns.add(o.name);
        }
      }
    }

    for (const owned of entity.owns) {
      if (parentOwns.has(owned.name)) continue;
      const attrClass = attrClassMap.get(owned.name);
      if (attrClass) referencedAttrClasses.add(attrClass);
    }
  }

  // Determine which factory symbols are needed
  const needsKey = sortedNames.some((entityName) => {
    const entity = schema.entities[entityName];
    if (!entity) return false;
    const parentOwns = buildParentOwns(entityName, schema);
    return entity.owns.some((owned) => {
      if (parentOwns.has(owned.name)) return false;
      return isKeyAttribute(owned.name, owned.is_key, options);
    });
  });

  const needsUnique = sortedNames.some((entityName) => {
    const entity = schema.entities[entityName];
    if (!entity) return false;
    const parentOwns = buildParentOwns(entityName, schema);
    return entity.owns.some(
      (owned) => !parentOwns.has(owned.name) && owned.is_unique,
    );
  });

  const needsCard = sortedNames.some((entityName) => {
    const entity = schema.entities[entityName];
    if (!entity) return false;
    const parentOwns = buildParentOwns(entityName, schema);
    return entity.owns.some((owned) => {
      if (parentOwns.has(owned.name)) return false;
      const card = owned.cardinality;
      return card !== null && isCardMulti(card);
    });
  });

  const needsTypeFlags = sortedNames.some((name) => schema.entities[name]?.is_abstract);

  const needsOrdered = sortedNames.some((entityName) => {
    const entity = schema.entities[entityName];
    if (!entity) return false;
    const parentOwns = buildParentOwns(entityName, schema);
    return entity.owns.some((owned) => !parentOwns.has(owned.name) && owned.ordered);
  });

  const needsDistinct = sortedNames.some((entityName) => {
    const entity = schema.entities[entityName];
    if (!entity) return false;
    const parentOwns = buildParentOwns(entityName, schema);
    return entity.owns.some((owned) => !parentOwns.has(owned.name) && owned.distinct);
  });

  // Build the factory import list
  const factoryImports: string[] = ["Entity", "field"];
  if (needsKey) factoryImports.push("Key");
  if (needsUnique) factoryImports.push("Unique");
  if (needsCard) factoryImports.push("Card");
  if (needsTypeFlags) factoryImports.push("TypeFlags");
  if (needsOrdered) factoryImports.push("Ordered");
  if (needsDistinct) factoryImports.push("Distinct");
  factoryImports.sort();

  const sortedAttrImports = [...referencedAttrClasses].sort();
  const lines: string[] = [`import { ${factoryImports.join(", ")} } from "@type-bridge/node";`];
  if (sortedAttrImports.length > 0) {
    lines.push(`import { ${sortedAttrImports.join(", ")} } from "./attributes.js";`);
  }
  lines.push(``);

  // Emit each entity class
  for (const entityName of sortedNames) {
    const entity = schema.entities[entityName];
    if (!entity) continue;

    const className = entityClassMap.get(entityName) ?? toClassName(entityName);

    // Determine own attrs (not inherited)
    const parentOwns = buildParentOwns(entityName, schema);

    // Use owns_order for deterministic field ordering (mirrors Python `entity.owns_order`)
    const ownLocalAttrs = entity.owns_order.filter((n) => !parentOwns.has(n));

    // Build the fields object entries
    const fieldEntries: string[] = [];
    for (const attrName of ownLocalAttrs) {
      const owned = entity.owns.find((o) => o.name === attrName);
      if (!owned) continue;
      const attrClass = attrClassMap.get(attrName);
      if (!attrClass) continue;

      const fieldKey = toFieldName(attrName);
      const isKey = isKeyAttribute(attrName, owned.is_key, options);
      const fieldExpr = renderFieldExpr(owned, attrClass, isKey);
      fieldEntries.push(`  ${fieldKey}: ${fieldExpr},`);
    }

    // First argument: TypeFlags({...}) for abstract, or plain string
    let firstArg: string;
    if (entity.is_abstract) {
      firstArg = `TypeFlags({ name: "${entityName}", abstract: true })`;
    } else {
      firstArg = `"${entityName}"`;
    }

    // Third argument: { parent: ParentClass } if parent present
    const parentClassName = entity.parent ? entityClassMap.get(entity.parent) : null;
    const thirdArg = parentClassName ? `, { parent: ${parentClassName} }` : "";

    if (fieldEntries.length === 0) {
      lines.push(`export class ${className} extends Entity(${firstArg}, {}${thirdArg}) {}`);
    } else {
      lines.push(`export class ${className} extends Entity(${firstArg}, {`);
      for (const entry of fieldEntries) {
        lines.push(entry);
      }
      lines.push(`}${thirdArg}) {}`);
    }
    lines.push(``);
  }

  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Build the set of attribute names owned by the direct parent entity. */
function buildParentOwns(entityName: string, schema: TypeSchema): Set<string> {
  const entity = schema.entities[entityName];
  if (!entity?.parent) return new Set();
  const parent = schema.entities[entity.parent];
  if (!parent) return new Set();
  return new Set(parent.owns.map((o) => o.name));
}
