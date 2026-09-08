from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PRODUCER = (
    ROOT
    / "type-bridge-core"
    / "crates"
    / "schema-codegen"
    / "tests"
    / "rust_acceptance"
    / "projected_parity.rs"
)
NEGATIVE = PRODUCER.with_name("projected_foreign_negative.rs")
HARNESS = PRODUCER.parents[1] / "rust_acceptance.rs"


def test_rust_projected_producer_uses_real_generated_projection_paths() -> None:
    source = PRODUCER.read_text()
    assert "RemoteDatabase::connect" in source
    assert "RemoteQueryResponseV2::new" in source
    assert "SCHEMA.generated_projection_validator()" in source
    assert "foreign::Person" in source
    assert "create_new(true)" in source
    assert "sync_all()" in source
    assert "materialize_model_for_test" not in source


def test_rust_projected_foreign_fence_is_derived_from_runtime_diagnostics() -> None:
    source = PRODUCER.read_text()
    assert "foreign::CounterCreate::new" in source
    assert "ProjectedCreate::try_new" in source
    assert ".validate_for(local)" in source
    assert "claimed.decode(&response)" in source
    assert "ProjectedQueryOrigin::remote_unbound()" in source
    assert "materialize_borrowed_with_budget" in source
    assert "thing.validate_for(local)" in source
    assert '"category": hydrated.foreign_fence.construction.category().as_str()' in source
    assert '"code": hydrated.foreign_fence.construction.code().as_str()' in source
    assert '"category": hydrated.foreign_fence.hydration.category().as_str()' in source
    assert '"code": hydrated.foreign_fence.hydration.code().as_str()' in source
    assert "generated_token_package_mismatch" not in source
    assert "SdkExecutionDiagnostic::generated_token_package_mismatch" not in source
    assert "ProjectedThing::try_new" not in source


def test_rust_projected_producer_does_not_import_expected_parity_evidence() -> None:
    source = PRODUCER.read_text()
    for forbidden in (
        "expected_report",
        "compare_projected_parity",
        'journey-v3.json")?',
        "batch_parity",
        "filter_parity",
    ):
        assert forbidden not in source


def test_rust_projected_harness_proves_nominal_foreign_fence_and_canonical_report() -> None:
    negative = NEGATIVE.read_text()
    harness = HARNESS.read_text()
    assert "foreign::PersonRef" in negative
    assert "generated::PlainActivityCreate" in negative
    assert "foreign::Person" in negative and "generated::Person" in negative
    assert "generated_rust_projected_parity_producer" in harness
    assert "_load_report" in harness
    assert "--offline" in harness
