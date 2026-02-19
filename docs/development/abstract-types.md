# Abstract Types and Interface Hierarchies in TypeDB

This document explains how abstract types, interface hierarchies, and polymorphic queries work in TypeDB 3.x.

## Table of Contents

1. [What Are Abstract Types?](#what-are-abstract-types)
2. [Interface Hierarchies](#interface-hierarchies)
3. [Abstract Types in TypeDB 3.x vs 2.x](#abstract-types-in-typedb-3x-vs-2x)
4. [Querying Abstract Types](#querying-abstract-types)

For TypeBridge implementation details, patterns, and known issues, see [docs/guide/abstract-types.md](../guide/abstract-types.md).

---

## What Are Abstract Types?

In TypeDB 3.x, **abstractness serves one primary purpose: making a type non-instantiable**. Abstract types cannot have direct instances created but serve as interface contracts that concrete subtypes must implement.

### Key Characteristics

- **Abstract types cannot be instantiated** - you cannot create direct instances
- **Abstract types CAN be queried polymorphically** - queries match all concrete subtypes
- **Abstract types define contracts** - they establish common attributes/roles that subtypes inherit
- **Granular control** - unlike TypeDB 2.x, you can choose which parts of an interface are abstract

### Example Schema

```typeql
# Abstract attribute type
attribute isbn @abstract, value string;

# Concrete subtypes
attribute isbn-10 sub isbn, value string;
attribute isbn-13 sub isbn, value string;

# Abstract entity type
entity book @abstract,
    owns isbn;

# Concrete subtypes
entity paperback sub book,
    owns isbn-10 as isbn;

entity hardback sub book,
    owns isbn-13 as isbn;
```

---

## Interface Hierarchies

TypeDB treats interfaces as first-class types that form their own hierarchies. This enables powerful polymorphic querying and prevents interface redundancies.

### Ownership Interfaces

When attributes form type hierarchies, their ownership interfaces inherit accordingly:

```typeql
attribute isbn @abstract, value string;
attribute isbn-10 sub isbn, value string;
attribute isbn-13 sub isbn, value string;

# Ownership interfaces:
# isbn:OWNER (abstract)
#   ├── isbn-10:OWNER
#   └── isbn-13:OWNER
```

### Role Interfaces

Relation type hierarchies create corresponding role interface hierarchies:

```typeql
relation contribution @abstract,
    relates contributor;

relation authoring sub contribution,
    relates author as contributor;

relation editing sub contribution,
    relates editor as contributor;

# Role interfaces:
# contribution:contributor (abstract)
#   ├── authoring:author (override)
#   └── editing:editor (override)
```

### Role Inheritance vs Override

**When to inherit roles:**
- Role players of the parent relation should also play roles in subtypes
- Example: All `contribution:contributor` instances can be `authoring:author`

**When to override roles:**
- Role players should be specialized to the subtype
- Overriding "acts like a combination of subtyping and making the inherited value abstract"
- Example: `employment:employee` overrides `relation:role-player` for specialization

---

## Abstract Types in TypeDB 3.x vs 2.x

### TypeDB 2.x: "Infectious Abstractness"

In TypeDB 2.x, abstractness was infectious - if a type was abstract, all types that referenced it were forced to be abstract as well. Non-abstract entities couldn't own abstract attributes.

### TypeDB 3.x: Granular Control

TypeDB 3.x provides **granular control** over which interface elements form contracts:

```typeql
# ✅ ALLOWED in TypeDB 3.x
entity book,  # Non-abstract entity
    owns isbn;  # Can own abstract attribute

# The entity is concrete, but it contracts to own
# the abstract isbn interface (implemented by isbn-10 or isbn-13)
```

**Key difference**: Non-abstract entities can now own abstract attributes, giving you precise control over interface contracts.

---

## Querying Abstract Types

### Polymorphic Queries

**You CAN query abstract types polymorphically** - the query matches all concrete subtypes:

```typeql
# Query abstract type - matches ALL concrete subtypes
match
$book isa book;  # Matches paperback, hardback, and any other book subtypes
```

### Exact Type Matching

Use `isa!` to match only the exact type (not subtypes):

```typeql
# Match only direct instances (won't match if book is abstract)
match
$book isa! book;

# With abstract types, this returns nothing since abstract types have no instances
```

### Attribute Polymorphism

```typeql
# Query abstract attribute - retrieves all concrete implementations
match
$book isa book, has isbn $isbn;

# This matches books with isbn-10 OR isbn-13
# The abstract type acts as a polymorphic umbrella
```

### Relation Polymorphism

```typeql
# Query abstract relation - matches all concrete subtypes
match
$contrib (contributor: $person) isa contribution;

# Matches authoring, editing, illustrating, etc.
```

---

## Further Reading

For more details on TypeDB concepts, see:
- [Using Interface Hierarchies](https://typedb.com/docs/academy/9-modeling-schemas/9.6-using-interface-hierarchies/)
- [Avoiding Interface Redundancies](https://typedb.com/docs/academy/9-modeling-schemas/9.7-avoiding-interface-redundancies/)
- [Abstract Contracts](https://typedb.com/docs/academy/9-modeling-schemas/9.8-abstract-contracts/)

For TypeBridge implementation details, see [docs/guide/abstract-types.md](../guide/abstract-types.md).
