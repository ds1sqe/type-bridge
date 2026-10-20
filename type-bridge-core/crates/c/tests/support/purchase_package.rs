//! The one-query purchase schema as an emitted C schema package, and the
//! hand-built purchase plan every SDK builder must reproduce (shared with the
//! Rust SDK, server and generated-crate tests).
#![allow(dead_code)]

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
use type_bridge_contract::query_plan::v3::QueryPlanV3;
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, VerifiedSchemaAuthority,
    build_schema_authority, normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

#[path = "../../../rust/tests/support/purchase_plan.rs"]
mod purchase_plan;
pub use purchase_plan::{PROFILE, SCOPE, SOURCE};

/// The generated C package symbol prefix (`shop_schema_package_open`).
pub const PREFIX: &str = "shop";

pub struct PurchasePackage {
    pub package: GeneratedPackage,
    pub authority: VerifiedSchemaAuthority,
}

/// Emit the purchase schema as a C package under `profile`, or the
/// acceptance profile when set, else the purchase fixture profile.
pub fn purchase_package() -> PurchasePackage {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("purchase.yaml").expect("document ID is valid"),
        SOURCE,
    )])
    .expect("purchase schema parses");
    let declared = normalize_documents(&documents).expect("purchase schema normalizes");
    let profile = SemanticProfileId::new(
        std::env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
            .unwrap_or_else(|_| PROFILE.to_owned()),
    )
    .expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("purchase schema resolves");
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(SCOPE).expect("scope is valid"),
        profile,
        available,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("purchase authority builds");
    let emitter = CEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new(PREFIX).expect("prefix is valid")),
        &emitter.generator_handlers(),
        &emitter.code_resources().expect("C emitter resources hash"),
    )
    .expect("purchase schema projects");
    let package = emitter
        .emit(&projection, &authority)
        .expect("purchase package emits");
    PurchasePackage { package, authority }
}

impl PurchasePackage {
    /// The hand-built purchase under this package's authority.
    pub fn hand_built(&self, order: &str, item: &str, account: &str) -> QueryPlanV3 {
        purchase_plan::hand_built_purchase(
            self.authority.declared_schema(),
            self.authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
            order,
            item,
            account,
        )
    }

    /// Generated source text of `path`.
    pub fn source(&self, path: &str) -> &str {
        std::str::from_utf8(self.package.get(path).expect("generated file exists"))
            .expect("generated file is UTF-8")
    }
}
