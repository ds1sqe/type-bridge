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

    Server deployment, logging, compatibility, deprecations, and 2.1 upgrade
    sequencing.

    [:octicons-arrow-right-24: Operations](operations.md)

</div>

## How the guides relate

Split-YAML is the authoring authority. Applications check one workspace, review
and apply canonical migrations, then generate Python, TypeScript, or Rust
projections. Historical inputs use the documented one-way conversion/adoption
paths; they are not parallel active authoring systems.

All runtime surfaces delegate semantic work to the shared Rust engine. The
[Python API reference](../reference/index.md) is generated from source
docstrings; the pages in this section explain workflows and contracts.
