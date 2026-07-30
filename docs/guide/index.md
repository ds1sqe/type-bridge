# Guides

TypeBridge combines language SDKs with schema, query, migration, generation,
and server workflows. These guides are grouped by the task you are performing,
while detailed reference pages keep their stable URLs.

<div class="grid cards" markdown>

-   **Choose an SDK**

    Python, TypeScript/Node, generated Rust, or remote server execution.

    [:octicons-arrow-right-24: Compare surfaces](sdks.md)

-   **Model TypeDB data**

    Attributes, entities, relations, roles, inheritance, cardinality, and
    validation.

    [:octicons-arrow-right-24: Model guide](models.md)

-   **Read and write data**

    CRUD managers, transactions, expressions, functions, and immutable typed
    queries.

    [:octicons-arrow-right-24: Data guide](data.md)

-   **Own the schema lifecycle**

    Canonical Split-YAML, TypeQL compatibility, migration, generation, and
    API projections.

    [:octicons-arrow-right-24: Schema workflows](schema-workflows.md)

-   **Operate and upgrade**

    Server deployment, logging, compatibility, deprecations, and 2.0 upgrade
    sequencing.

    [:octicons-arrow-right-24: Operations](operations.md)

</div>

## How the guides relate

Application models and canonical schemas describe the same TypeDB concepts.
Choose one authoring authority for a scope:

- model-first Python applications can register classes and use
  `SchemaManager` during the 2.0 compatibility window;
- schema-first applications author Split-YAML, plan migrations, and generate
  Python, TypeScript, or Rust projections;
- existing TypeQL applications can generate models through the retained
  compatibility path.

All runtime surfaces delegate semantic work to the shared Rust engine. The
[Python API reference](../reference/index.md) is generated from source
docstrings; the pages in this section explain workflows and contracts.
