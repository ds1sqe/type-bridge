#![allow(dead_code)]

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, VerifiedSchemaAuthority,
    build_schema_authority, normalize_documents,
};

pub const TEST_PROFILE: &str = "typedb-3.12.1/v1";
pub const TEST_SCOPE: &str = "schema-codegen-acceptance";

pub fn declared(source: &str) -> DeclaredSchema {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema-codegen-authority.yaml").expect("test document ID"),
        source,
    )])
    .expect("test authority source parses");
    normalize_documents(&documents).expect("test authority source normalizes")
}

pub fn authority(source: &str) -> VerifiedSchemaAuthority {
    authority_with(source, TEST_SCOPE, TEST_PROFILE)
}

pub fn authority_with(source: &str, scope: &str, profile: &str) -> VerifiedSchemaAuthority {
    let declared = declared(source);
    authority_for_declared(&declared, scope, profile)
}

pub fn authority_for_declared(
    declared: &DeclaredSchema,
    scope: &str,
    profile: &str,
) -> VerifiedSchemaAuthority {
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("test managed scope"),
        SemanticProfileId::new(profile).expect("test semantic profile"),
        available,
    );
    build_schema_authority(declared, declared.required_capabilities(), &context)
        .expect("test schema authority builds")
}
