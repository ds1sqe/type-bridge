//! Lossless schema documents and pure schema normalization.
//!
//! This crate owns source-oriented schema concerns so YAML parsing never enters
//! the protocol-level `type-bridge-contract` dependency graph.

mod adoption;
mod assembler;
mod delta;
mod delta_dependencies;
mod delta_safety;
mod diagnostic;
mod discovery;
mod document;
mod normalize;
mod observed;
mod project;
mod resolve;
mod safety_condition;
mod schema_set;
mod semantic;
mod source_pattern;
mod timezone;
mod yaml;

pub use adoption::{AdoptionBaseline, adopt_observed_schema};
pub use assembler::FactAssembler;
pub use delta::{
    DeltaError, ManagedDeltaContext, apply_delta, diff_managed, inverse_delta, managed_schema_state,
};
pub use delta_dependencies::{FactDependencyGraph, plan_schema_operations};
pub use delta_safety::{
    DeltaSafety, DeltaSafetyReason, DeltaSafetyReport, SafetyClass, SafetyClassificationError,
    classify_delta_safety, classify_operation_safety, classify_schema_operation_safety,
};
pub use discovery::{
    DEFAULT_MAX_DISCOVERY_DEPTH, DEFAULT_MAX_DISCOVERY_ENTRIES, DEFAULT_MAX_SOURCE_PATTERN_BYTES,
    DEFAULT_MAX_SOURCE_PATTERNS, SchemaDiscoveryEvidence, SchemaDiscoveryLimits,
    SchemaDiscoverySnapshot, SchemaPatternDiscoverySnapshot, SchemaSourceCapture,
    SchemaSourceEvidence, SchemaSourceIdentity, SchemaSourceKind, SchemaSourceObservation,
    SchemaSourceRevision, SchemaSourceService, SchemaSourceServiceError, SystemSchemaSourceService,
    discover_schema_documents, discover_schema_documents_with_limits, load_schema_set,
    load_schema_set_with_limits, load_schema_set_with_source,
};
pub use document::{
    CommentPlacement, SchemaComment, SchemaDocument, SchemaDocumentSet, SchemaParseLimits,
    YamlCollectionStyle, YamlMapping, YamlMappingEntry, YamlNode, YamlScalar, YamlScalarStyle,
    YamlSequence,
};
pub use normalize::{SCHEMA_V2_FORMAT, normalize_documents};
pub use observed::{
    CanonicalObservedSchema, OBSERVED_SCHEMA_CANONICALIZATION_VERSION, ObservedFactProvenance,
    ObservedFactScope, ObservedSchema, ObservedSchemaFact, canonicalize_observed_schema,
};
pub use project::project;
pub use resolve::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, DescriptorId, DescriptorIndex, EffectiveOwns, EffectivePlays,
    EffectiveRelates, EffectiveRelatesId, EffectiveSub, EffectiveValueType, ResolutionOrigin,
    ResolvedFunction, ResolvedRole, ResolvedSchema, ResolvedStruct, ResolvedType,
    SchemaDependencyGraph, resolve, resolve_schema_with_capabilities,
};
pub use safety_condition::{
    DerivedSafetyConditions, RequiredSafetyCondition, SAFETY_CONDITION_CANONICALIZATION,
    SAFETY_CONDITION_FINGERPRINT_DOMAIN, SafetyCondition, SafetyConditionId, SafetyConditionUnlock,
    SafetyDerivationProfile, ScalarSafetySubject, UnresolvableSafetyReason,
    derive_safety_conditions,
};
pub use schema_set::{
    SCHEMA_DISCOVERY_V1, SCHEMA_SET_V1_FORMAT, SchemaDiscoveryVersion, SchemaSetManifest,
    SchemaSetManifestDocument,
};
pub use semantic::{
    BoundManagedSchemaScope, ManagedSchemaScope, canonical_managed_declared_identity_bytes,
    canonical_managed_semantic_schema_bytes, canonical_semantic_schema_bytes,
    managed_declared_identity_fingerprint, managed_semantic_schema_fingerprint,
    semantic_schema_fingerprint,
};
pub use timezone::{
    TYPEDB_3_12_1_TEMPORAL_POLICY_ID, TYPEDB_3_12_1_TIMEZONE_POLICY_ID, parse_provider_datetime_tz,
    parse_provider_datetime_tz_evidence, resolve_provider_datetime_tz,
    validate_provider_datetime_tz, validate_provider_duration, validate_provider_temporal_literal,
    validate_provider_temporal_value,
};
