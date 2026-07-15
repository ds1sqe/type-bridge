use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyModule};
use serde::{Deserialize, Serialize};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain,
};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::value::{CanonicalValue, Cardinality};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FoundationProbe {
    capabilities: CapabilitySet,
    cardinality: Cardinality,
    fingerprint: Fingerprint,
    long: CanonicalValue,
    type_id: TypeId,
}

#[pyfunction]
fn round_trip_contract_bytes(
    py: Python<'_>,
    payload: &Bound<'_, PyBytes>,
) -> PyResult<Py<PyBytes>> {
    let decoded: FoundationProbe = from_canonical_json(payload.as_bytes())
        .map_err(|error| PyValueError::new_err(format!("{}: {error}", error.code())))?;
    let encoded = to_canonical_json(&decoded)
        .map_err(|error| PyValueError::new_err(format!("{}: {error}", error.code())))?;
    Ok(PyBytes::new(py, &encoded).unbind())
}

fn probe() -> FoundationProbe {
    let long = CanonicalValue::Long(9_007_199_254_740_993);
    let long_bytes = to_canonical_json(&long).unwrap();
    FoundationProbe {
        capabilities: CapabilitySet::from_iter([
            CapabilityId::new("schema.annotations").unwrap(),
            CapabilityId::new("query.given-multi-row").unwrap(),
        ]),
        cardinality: Cardinality::new(0, None).unwrap(),
        fingerprint: Fingerprint::compute(
            FingerprintDomain::new("test.value").unwrap(),
            CanonicalizationVersion::new("typebridge.canonical-json/v1").unwrap(),
            None,
            &long_bytes,
        ),
        long,
        type_id: TypeId::new(TypeKind::Entity, "person").unwrap(),
    }
}

#[test]
fn foundation_bytes_round_trip_through_an_in_memory_python_module() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| -> PyResult<()> {
        let expected = include_bytes!("../../contract/tests/fixtures/foundation-probe-v1.json");
        let expected = expected.strip_suffix(b"\n").unwrap_or(expected);
        let original = probe();
        let encoded = to_canonical_json(&original)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        assert_eq!(encoded, expected);

        let module = PyModule::new(py, "_type_bridge_contract_probe")?;
        module.add_function(wrap_pyfunction!(round_trip_contract_bytes, &module)?)?;
        let returned = module
            .getattr("round_trip_contract_bytes")?
            .call1((PyBytes::new(py, &encoded),))?;
        let returned: Vec<u8> = returned.extract()?;
        assert_eq!(returned, encoded);
        assert_eq!(from_canonical_json::<FoundationProbe>(&returned).unwrap(), original);
        Ok(())
    })
    .unwrap();
}
