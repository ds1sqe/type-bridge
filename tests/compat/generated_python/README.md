# Generated Python Artifact Consumer

This fixture executes schema-codegen's generated Python acceptance package
against extracted candidate wheels. It verifies runtime construction/query
behavior, exact positive and negative Pyright contracts, package isolation,
and the absence of handwritten root authoring exports.

The CI and release workflows invoke it through
`scripts/ci/run_generated_python_artifact.py`; it does not build either wheel.
