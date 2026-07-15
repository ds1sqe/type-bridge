//! Feature-gated native smoke adapter for Phase 1 contract bytes.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::fingerprint::Fingerprint;
use type_bridge_contract::id::TypeId;
use type_bridge_contract::value::{CanonicalValue, Cardinality};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ContractFoundationProbe {
    capabilities: CapabilitySet,
    cardinality: Cardinality,
    fingerprint: Fingerprint,
    long: CanonicalValue,
    type_id: TypeId,
}

/// Round-trip exact canonical foundation bytes through the N-API boundary.
///
/// This symbol exists only in builds that explicitly enable the private
/// `contract-test-adapter` feature.
#[napi(js_name = "__roundTripContractFoundation")]
pub fn round_trip_contract_foundation(input: Buffer) -> Result<Buffer> {
    let probe: ContractFoundationProbe =
        from_canonical_json(input.as_ref()).map_err(contract_error)?;
    if !matches!(probe.long, CanonicalValue::Long(_)) {
        return Err(Error::new(
            Status::InvalidArg,
            "contract foundation probe requires a tagged long",
        ));
    }
    to_canonical_json(&probe)
        .map(Buffer::from)
        .map_err(contract_error)
}

fn contract_error(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    let payload = serde_json::to_string(&error).unwrap_or_else(|_| {
        format!(r#"{{"code":"{}","message":"{}"}}"#, error.code(), error)
    });
    Error::new(Status::InvalidArg, payload)
}
