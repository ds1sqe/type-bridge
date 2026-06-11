/**
 * Render the `relations.ts` module text from a parsed TypeSchema.
 *
 * Mirrors `type_bridge/generator/render/relations.py` → `render_relations()`.
 *
 * Role-player derivation:
 *   Scan ALL entity `plays[]` entries for `role_ref === "<relation>:<role>"`.
 *   Remove ancestor types when a descendant is also present (minimal set).
 *   This is the exact algorithm of `minimal_role_players()` in models.py.
 *
 * Cardinality sources:
 *   Relates-side cardinality comes from the relation role definition
 *   (`RoleSpec.cardinality`). Plays-side cardinality comes from the player's
 *   `PlayedRole.cardinality` and renders as `playsCardinality` on `role(...)`.
 *
 * Field key = toFieldName(role.name) / toFieldName(attr.name).
 * Class names = toClassName(typeName) for all players / attr types.
 *
 * Build-time code generation; no runtime ORM logic.
 */

import type { TypeSchema, RoleSpec, Cardinality } from "../parser.js";
import { toClassName, toFieldName, isKeyAttribute, type NamingOptions } from "./naming.js";
import { isCardSingle, isCardRequired, isCardMulti, isCardOptionalSingle } from "./cardinality.js";

// ---------------------------------------------------------------------------
// Card() argument renderer — mirrors Python `_render_card_arg`
//
// Default (1,1) emits nothing — caller wraps with `{ cardinality: Card(...) }`.
// We always emit cardinality for roles (the hand-authored parity file always
// uses explicit Card()), but skip if it is the default single (1,1).
// ---------------------------------------------------------------------------

function renderCardArg(card: Cardinality | null, omitDefaultSingle = true): string {
  if (card === null) return "";
  if (omitDefaultSingle && card.min === 1 && card.max === 1) return "";
  const maxStr = card.max === null ? "null" : String(card.max);
  return `Card(${card.min}, ${maxStr})`;
}

// ---------------------------------------------------------------------------
// role() call renderer — mirrors `_render_role_field` in relations.py
// ---------------------------------------------------------------------------

function renderRoleCall(
  players: string[],
  card: Cardinality | null,
  playsCardinality: Cardinality | null,
  overrides: string | null = null,
): string {
  const cardArg = renderCardArg(card);
  const playsCardArg = renderCardArg(playsCardinality, false);
  const options: string[] = [];
  if (cardArg) options.push(`cardinality: ${cardArg}`);
  if (playsCardArg) options.push(`playsCardinality: ${playsCardArg}`);
  if (overrides != null) options.push(`overrides: "${overrides}"`);
  const cardOptions = options.length > 0 ? `{ ${options.join(", ")} }` : null;

  if (players.length === 1) {
    const args = cardOptions ? `${players[0]!}, ${cardOptions}` : players[0]!;
    return `role(${args})`;
  }

  // Multi-player: role(A, B, ..., { cardinality: ... })
  const playerArgs = players.join(", ");
  const args = cardOptions ? `${playerArgs}, ${cardOptions}` : playerArgs;
  return `role(${args})`;
}

function playsCardinalityForRole(
  schema: TypeSchema,
  relationName: string,
  roleName: string,
  players: string[],
): Cardinality | null {
  const roleToken = `${relationName}:${roleName}`;
  for (const player of players) {
    const entity = schema.entities[player];
    const played = entity?.plays.find((entry) => entry.role_ref === roleToken);
    if (played?.cardinality !== null && played?.cardinality !== undefined) {
      return played.cardinality;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Minimal role players — mirrors `minimal_role_players` in models.py
//
// Scan entity plays[] for role_ref === "<relationName>:<roleName>".
// Then remove ancestors when a descendant is also present.
// ---------------------------------------------------------------------------

function minimalRolePlayers(
  schema: TypeSchema,
  relationName: string,
  roleName: string,
): string[] {
  const roleToken = `${relationName}:${roleName}`;

  // Collect all entities that play this role
  const players: string[] = [];
  for (const [entityName, entity] of Object.entries(schema.entities)) {
    for (const played of entity.plays) {
      if (played.role_ref === roleToken) {
        players.push(entityName);
        break;
      }
    }
  }

  if (players.length === 0) return [];

  // Build parent map
  const parentMap = new Map<string, string | null>();
  for (const [name, entity] of Object.entries(schema.entities)) {
    parentMap.set(name, entity.parent);
  }

  function isAncestor(candidate: string, target: string): boolean {
    let current = parentMap.get(target) ?? null;
    while (current !== null) {
      if (current === candidate) return true;
      current = parentMap.get(current) ?? null;
    }
    return false;
  }

  // Remove ancestors when a descendant is also present
  const unique = new Set(players);
  const minimal = new Set(unique);
  for (const player of unique) {
    for (const other of unique) {
      if (player !== other && isAncestor(other, player) && minimal.has(player)) {
        minimal.delete(player);
        break;
      }
    }
  }

  return [...minimal].sort();
}

// ---------------------------------------------------------------------------
// Topological sort — parents before children (mirrors `_topological_sort_relations`)
// ---------------------------------------------------------------------------

function topologicalSortRelations(schema: TypeSchema): string[] {
  const relations = schema.relations;
  const names = Object.keys(relations);
  const sorted: string[] = [];
  const visited = new Set<string>();

  function visit(name: string): void {
    if (visited.has(name)) return;
    visited.add(name);
    const parent = relations[name]?.parent;
    if (parent && relations[parent]) {
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
// Import need detectors
// ---------------------------------------------------------------------------

function needsCardImport(schema: TypeSchema): boolean {
  for (const relation of Object.values(schema.relations)) {
    // Check role cardinalities
    for (const role of relation.roles) {
      if (role.cardinality !== null) {
        const c = role.cardinality;
        if (!(c.min === 1 && c.max === 1)) return true;
      }
      const players = minimalRolePlayers(schema, relation.name, role.name);
      if (playsCardinalityForRole(schema, relation.name, role.name, players) !== null) {
        return true;
      }
    }
    // Check attribute cardinalities (multi-valued)
    for (const owned of relation.owns) {
      const c = owned.cardinality;
      if (c !== null && isCardMulti(c)) return true;
    }
  }
  return false;
}

function needsKeyImport(schema: TypeSchema, options?: NamingOptions): boolean {
  return Object.values(schema.relations).some((rel) =>
    rel.owns.some((o) => isKeyAttribute(o.name, o.is_key, options)),
  );
}

function needsUniqueImport(schema: TypeSchema): boolean {
  return Object.values(schema.relations).some((rel) =>
    rel.owns.some((o) => o.is_unique),
  );
}

function needsTypeFlagsImport(schema: TypeSchema): boolean {
  return Object.values(schema.relations).some((rel) => rel.is_abstract);
}

// ---------------------------------------------------------------------------
// Single-field renderer for owned attributes (mirrors renderEntities.ts logic)
// ---------------------------------------------------------------------------

function renderAttrFieldExpr(
  owned: { cardinality: Cardinality | null; is_unique: boolean },
  attrClassName: string,
  isKey: boolean,
): string {
  const card = owned.cardinality;

  if (isKey) return `field(${attrClassName}, Key)`;
  if (owned.is_unique) return `field(${attrClassName}, Unique)`;
  if (card === null || isCardOptionalSingle(card)) return `field(${attrClassName}).optional()`;
  if (isCardMulti(card)) {
    const maxStr = card.max === null ? "null" : String(card.max);
    return `field(${attrClassName}).list(Card(${card.min}, ${maxStr}))`;
  }
  if (isCardRequired(card) && isCardSingle(card)) return `field(${attrClassName})`;
  return `field(${attrClassName}).optional()`;
}

// ---------------------------------------------------------------------------
// Main renderer
// ---------------------------------------------------------------------------

/**
 * Render the complete `relations.ts` source text from the parsed schema.
 *
 * Each relation emits:
 *   export class <ClassName> extends Relation("<name>", { <fields> }) {}
 *
 * Role keys = toFieldName(role.name).
 * Attr keys = toFieldName(attr.name).
 * Player names = toClassName(entityName).
 *
 * Imports: `@type-bridge/node` for factory symbols; `./attributes.js` for
 * attribute classes; `./entities.js` for player entity classes.
 */
export function renderRelations(schema: TypeSchema, options?: NamingOptions): string {
  const sortedNames = topologicalSortRelations(schema);

  // Pre-compute class name maps
  const attrClassMap = new Map<string, string>();
  for (const name of Object.keys(schema.attributes)) {
    attrClassMap.set(name, toClassName(name));
  }

  const entityClassMap = new Map<string, string>();
  for (const name of Object.keys(schema.entities)) {
    entityClassMap.set(name, toClassName(name));
  }

  const relationClassMap = new Map<string, string>();
  for (const name of Object.keys(schema.relations)) {
    relationClassMap.set(name, toClassName(name));
  }

  // Determine which factory symbols are needed
  const needsCard = needsCardImport(schema);
  const needsKey = needsKeyImport(schema, options);
  const needsUnique = needsUniqueImport(schema);
  const needsTypeFlags = needsTypeFlagsImport(schema);

  const factoryImports: string[] = ["Relation", "role", "field"];
  if (needsCard) factoryImports.push("Card");
  if (needsKey) factoryImports.push("Key");
  if (needsUnique) factoryImports.push("Unique");
  if (needsTypeFlags) factoryImports.push("TypeFlags");
  factoryImports.sort();

  // Collect which attribute + entity class names are referenced (for imports)
  const referencedAttrClasses = new Set<string>();
  const referencedEntityClasses = new Set<string>();

  // Two-pass: collect all referenced classes first
  for (const relName of sortedNames) {
    const relation = schema.relations[relName];
    if (!relation) continue;

    // Parent-owned roles (skip inherited, same as Python)
    const parentRoleNames = new Set<string>();
    if (relation.parent && schema.relations[relation.parent]) {
      for (const r of schema.relations[relation.parent]!.roles) {
        parentRoleNames.add(r.name);
      }
    }

    // Roles
    for (const role of relation.roles) {
      if (parentRoleNames.has(role.name) && !role.overrides) continue;
      const players = minimalRolePlayers(schema, relName, role.name);
      for (const p of players) {
        const cls = entityClassMap.get(p);
        if (cls) referencedEntityClasses.add(cls);
      }
    }

    // Attributes (only own, not inherited)
    const parentOwns = new Set<string>();
    if (relation.parent && schema.relations[relation.parent]) {
      for (const o of schema.relations[relation.parent]!.owns) {
        parentOwns.add(o.name);
      }
    }

    for (const owned of relation.owns) {
      if (parentOwns.has(owned.name)) continue;
      const cls = attrClassMap.get(owned.name);
      if (cls) referencedAttrClasses.add(cls);
    }
  }

  // Build import lines
  const lines: string[] = [
    `import { ${factoryImports.join(", ")} } from "@type-bridge/node";`,
  ];

  const sortedAttrImports = [...referencedAttrClasses].sort();
  if (sortedAttrImports.length > 0) {
    lines.push(`import { ${sortedAttrImports.join(", ")} } from "./attributes.js";`);
  }

  const sortedEntityImports = [...referencedEntityClasses].sort();
  if (sortedEntityImports.length > 0) {
    lines.push(`import { ${sortedEntityImports.join(", ")} } from "./entities.js";`);
  }

  lines.push(``);

  // Emit each relation class
  for (const relName of sortedNames) {
    const relation = schema.relations[relName];
    if (!relation) continue;

    const className = relationClassMap.get(relName) ?? toClassName(relName);

    // First argument
    let firstArg: string;
    if (relation.is_abstract) {
      firstArg = `TypeFlags({ name: "${relName}", abstract: true })`;
    } else {
      firstArg = `"${relName}"`;
    }

    // Third argument: { parent: ParentClass }
    const parentClassName = relation.parent ? relationClassMap.get(relation.parent) : null;
    const thirdArg = parentClassName ? `, { parent: ${parentClassName} }` : "";

    // Collect parent-owned role names + attr names (skip inherited)
    const parentRoleNames = new Set<string>();
    const parentOwns = new Set<string>();
    if (relation.parent && schema.relations[relation.parent]) {
      const parentRel = schema.relations[relation.parent]!;
      for (const r of parentRel.roles) {
        parentRoleNames.add(r.name);
      }
      for (const o of parentRel.owns) {
        parentOwns.add(o.name);
      }
    }

    // Build field entries: roles first, then attrs (mirror Python relation template order)
    const fieldEntries: string[] = [];

    // Role fields
    for (const roleSpec of relation.roles) {
      if (parentRoleNames.has(roleSpec.name) && !roleSpec.overrides) continue;

      const players = minimalRolePlayers(schema, relName, roleSpec.name);
      if (players.length === 0) continue;

      const playerClassRefs = players.map(
        (p) => entityClassMap.get(p) ?? toClassName(p),
      );

      const roleKey = toFieldName(roleSpec.name);
      const playsCardinality = playsCardinalityForRole(
        schema,
        relName,
        roleSpec.name,
        players,
      );
      const roleCall = renderRoleCall(
        playerClassRefs,
        roleSpec.cardinality,
        playsCardinality,
        roleSpec.overrides,
      );
      fieldEntries.push(`  ${roleKey}: ${roleCall},`);
    }

    // Attribute fields (own only, in owns_order)
    const ownLocalAttrs = relation.owns_order.filter((n) => !parentOwns.has(n));
    for (const attrName of ownLocalAttrs) {
      const owned = relation.owns.find((o) => o.name === attrName);
      if (!owned) continue;
      const attrClass = attrClassMap.get(attrName);
      if (!attrClass) continue;

      const fieldKey = toFieldName(attrName);
      const isKey = isKeyAttribute(attrName, owned.is_key, options);
      const fieldExpr = renderAttrFieldExpr(owned, attrClass, isKey);
      fieldEntries.push(`  ${fieldKey}: ${fieldExpr},`);
    }

    if (fieldEntries.length === 0) {
      lines.push(`export class ${className} extends Relation(${firstArg}, {}${thirdArg}) {}`);
    } else {
      lines.push(`export class ${className} extends Relation(${firstArg}, {`);
      for (const entry of fieldEntries) {
        lines.push(entry);
      }
      lines.push(`}${thirdArg}) {}`);
    }
    lines.push(``);
  }

  return lines.join("\n");
}
