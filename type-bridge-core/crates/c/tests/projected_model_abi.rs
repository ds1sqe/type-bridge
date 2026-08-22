use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{self, NonNull};

use type_bridge_c::{
    GENERATED_CREATE_GRAPH_VERSION, GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX,
    GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE, GENERATED_CREATE_MEMBER_VALUE_SEQUENCE,
    GENERATED_INPUT_CREATE_ARGS_GRAPH, GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
    GENERATED_INPUT_SCHEMA_PACKAGE, TypeBridgeByteView, TypeBridgeCancellation,
    TypeBridgeCanonicalArchive, TypeBridgeCanonicalArchiveBuilder, TypeBridgeCanonicalBytes,
    TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticDetailKind,
    TypeBridgeExecutionDiagnosticDetailViewV1, TypeBridgeExecutionDiagnosticPathKind,
    TypeBridgeExecutionDiagnosticPathViewV1, TypeBridgeExecutionDiagnosticViewV1,
    TypeBridgeExecutionDiagnostics, TypeBridgeGeneratedCreateArgsGraphV1,
    TypeBridgeGeneratedCreateHandleChunkV1, TypeBridgeGeneratedCreateMemberV1,
    TypeBridgeGeneratedOpaqueInputV1, TypeBridgeGeneratedOutputRangeV1,
    TypeBridgeProjectedCodecOptionsV1, TypeBridgeProjectedCreate,
    TypeBridgeProjectedCreateDescriptorV1, TypeBridgeProjectedFieldInputV1,
    TypeBridgeProjectedReference, TypeBridgeProjectedReferenceDescriptorV1,
    TypeBridgeProjectedRoleInputV1, TypeBridgeProjectedStruct, TypeBridgeProjectedStructMember,
    TypeBridgeProjectedThing, TypeBridgeProjectedThingDescriptorV1, TypeBridgeProjectedTokenV1,
    TypeBridgeProjectedValue, TypeBridgeProjectedValueKind, TypeBridgeSchemaPackage,
    TypeBridgeSchemaPackageDescriptorV1, TypeBridgeStatus,
};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectedTokenIdentity, ProjectionConfig, RuntimeProjection,
    TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
};
use type_bridge_contract::schema::{
    DeclaredIdentityFingerprint, DocumentId, OwnsFactId, encode_declared_schema,
};
use type_bridge_contract::value::CanonicalValue;
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, build_schema_authority,
    encode_schema_authority, normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::CEmitter;

const SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  secondary-identifier: { value: string }
  measure-long:
    value:
      type: integer
      range: { min: -10, max: 10 }
  measure-double: { value: double }
  enabled: { value: boolean }
  born-on: { value: date }
  observed-at: { value: datetime }
  zoned-at: { value: datetime-tz }
  balance: { value: decimal }
  elapsed: { value: duration }
entities:
  person:
    owns:
      identifier: { key: true }
      measure-long: {}
      measure-double: {}
      enabled: {}
      born-on: {}
      observed-at: {}
      zoned-at: {}
      balance: {}
      elapsed: {}
relations:
  container:
    relates: [item]
  membership:
    relates:
      member: { card: 1 }
  optional-membership:
    relates:
      member: { card: { min: 0, max: 1 } }
  team:
    relates:
      participant: { card: 1 }
plays:
  person:
    membership: [member]
    optional-membership: [member]
    team: [participant]
structs:
  score-card:
    fields:
      - { name: score, type: integer }
      - { name: note, type: string, optional: true }
"#;

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn type_bridge_cancellation_open(
        out_cancellation: *mut *mut TypeBridgeCancellation,
    ) -> TypeBridgeStatus;
    fn type_bridge_cancellation_request(
        cancellation: *const TypeBridgeCancellation,
    ) -> TypeBridgeStatus;
    fn type_bridge_cancellation_close(
        cancellation: *mut *mut TypeBridgeCancellation,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_open_v1(
        descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut type_bridge_c::TypeBridgeDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_close(
        package: *mut *mut TypeBridgeSchemaPackage,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_count(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        out_count: *mut usize,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        index: usize,
        out_diagnostic: *mut TypeBridgeExecutionDiagnosticViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_path_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        diagnostic_index: usize,
        path_index: usize,
        out_path: *mut TypeBridgeExecutionDiagnosticPathViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_detail_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        diagnostic_index: usize,
        detail_index: usize,
        out_detail: *mut TypeBridgeExecutionDiagnosticDetailViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_close(
        diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_generated_opaque_alias_preflight_v1(
        inputs: *const TypeBridgeGeneratedOpaqueInputV1,
        input_count: usize,
        outputs: *const TypeBridgeGeneratedOutputRangeV1,
        output_count: usize,
    ) -> TypeBridgeStatus;

    fn type_bridge_projected_value_string_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: TypeBridgeByteView,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_long_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: i64,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_double_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: u64,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_boolean_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: u8,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_date_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: TypeBridgeByteView,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_datetime_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: TypeBridgeByteView,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_datetime_tz_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: TypeBridgeByteView,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_decimal_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: TypeBridgeByteView,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_duration_open(
        package: *const TypeBridgeSchemaPackage,
        token: *const TypeBridgeProjectedTokenV1,
        input: TypeBridgeByteView,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_kind(
        value: *const TypeBridgeProjectedValue,
        out_kind: *mut TypeBridgeProjectedValueKind,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_validate_model(
        value: *const TypeBridgeProjectedValue,
        model: *const TypeBridgeProjectedTokenV1,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_text(
        value: *const TypeBridgeProjectedValue,
        out_text: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_long(
        value: *const TypeBridgeProjectedValue,
        out_long: *mut i64,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_double_bits(
        value: *const TypeBridgeProjectedValue,
        out_bits: *mut u64,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_boolean(
        value: *const TypeBridgeProjectedValue,
        out_boolean: *mut u8,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_value_close(
        value: *mut *mut TypeBridgeProjectedValue,
    ) -> TypeBridgeStatus;

    fn type_bridge_projected_reference_open_v1(
        package: *const TypeBridgeSchemaPackage,
        descriptor: *const TypeBridgeProjectedReferenceDescriptorV1,
        out_reference: *mut *mut TypeBridgeProjectedReference,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_iid(
        reference: *const TypeBridgeProjectedReference,
        out_iid: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_validate_model(
        reference: *const TypeBridgeProjectedReference,
        model: *const TypeBridgeProjectedTokenV1,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_validate_role(
        reference: *const TypeBridgeProjectedReference,
        role: *const TypeBridgeProjectedTokenV1,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_clone(
        reference: *const TypeBridgeProjectedReference,
        out_reference: *mut *mut TypeBridgeProjectedReference,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_model_ordinal(
        reference: *const TypeBridgeProjectedReference,
        out_ordinal: *mut u32,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_key(
        reference: *const TypeBridgeProjectedReference,
        field: *const TypeBridgeProjectedTokenV1,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_reference_close(
        reference: *mut *mut TypeBridgeProjectedReference,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_create_open_v1(
        package: *const TypeBridgeSchemaPackage,
        descriptor: *const TypeBridgeProjectedCreateDescriptorV1,
        out_create: *mut *mut TypeBridgeProjectedCreate,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_create_field_count(
        create: *const TypeBridgeProjectedCreate,
        field: *const TypeBridgeProjectedTokenV1,
        out_count: *mut usize,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_create_field_value_at(
        create: *const TypeBridgeProjectedCreate,
        field: *const TypeBridgeProjectedTokenV1,
        index: usize,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_create_role_count(
        create: *const TypeBridgeProjectedCreate,
        role: *const TypeBridgeProjectedTokenV1,
        out_count: *mut usize,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_create_role_reference_at(
        create: *const TypeBridgeProjectedCreate,
        role: *const TypeBridgeProjectedTokenV1,
        index: usize,
        out_reference: *mut *mut TypeBridgeProjectedReference,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_create_close(
        create: *mut *mut TypeBridgeProjectedCreate,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_open_v1(
        package: *const TypeBridgeSchemaPackage,
        descriptor: *const TypeBridgeProjectedThingDescriptorV1,
        out_thing: *mut *mut TypeBridgeProjectedThing,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_record_encode_create_v1(
        value: *const TypeBridgeProjectedCreate,
        options: *const TypeBridgeProjectedCodecOptionsV1,
        out_bytes: *mut *mut TypeBridgeCanonicalBytes,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_record_decode_create_v1(
        package: *const TypeBridgeSchemaPackage,
        bytes: TypeBridgeByteView,
        expected_model: *const TypeBridgeProjectedTokenV1,
        options: *const TypeBridgeProjectedCodecOptionsV1,
        out_value: *mut *mut TypeBridgeProjectedCreate,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_record_decode_struct_v1(
        package: *const TypeBridgeSchemaPackage,
        bytes: TypeBridgeByteView,
        expected_struct: *const TypeBridgeProjectedTokenV1,
        options: *const TypeBridgeProjectedCodecOptionsV1,
        out_value: *mut *mut TypeBridgeProjectedStruct,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_struct_member_at_v1(
        value: *const TypeBridgeProjectedStruct,
        expected_struct: *const TypeBridgeProjectedTokenV1,
        index: usize,
        out_member: *mut *mut TypeBridgeProjectedStructMember,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_struct_member_long(
        value: *const TypeBridgeProjectedStructMember,
        out_value: *mut i64,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_struct_member_close(
        value: *mut *mut TypeBridgeProjectedStructMember,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_struct_close(
        value: *mut *mut TypeBridgeProjectedStruct,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_bytes_view(
        bytes: *const TypeBridgeCanonicalBytes,
        out_view: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_bytes_close(
        bytes: *mut *mut TypeBridgeCanonicalBytes,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_builder_open_v1(
        package: *const TypeBridgeSchemaPackage,
        options: *const TypeBridgeProjectedCodecOptionsV1,
        out_builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_builder_append_record_v1(
        builder: *mut TypeBridgeCanonicalArchiveBuilder,
        record: TypeBridgeByteView,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_builder_finish_v1(
        builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
        out_bytes: *mut *mut TypeBridgeCanonicalBytes,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_builder_close(
        builder: *mut *mut TypeBridgeCanonicalArchiveBuilder,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_open_v1(
        package: *const TypeBridgeSchemaPackage,
        bytes: TypeBridgeByteView,
        options: *const TypeBridgeProjectedCodecOptionsV1,
        out_archive: *mut *mut TypeBridgeCanonicalArchive,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_count(
        archive: *const TypeBridgeCanonicalArchive,
        out_count: *mut usize,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_record_at(
        archive: *const TypeBridgeCanonicalArchive,
        index: usize,
        out_bytes: *mut *mut TypeBridgeCanonicalBytes,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_canonical_archive_close(
        archive: *mut *mut TypeBridgeCanonicalArchive,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_iid(
        thing: *const TypeBridgeProjectedThing,
        out_iid: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_validate_model(
        thing: *const TypeBridgeProjectedThing,
        model: *const TypeBridgeProjectedTokenV1,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_reference(
        thing: *const TypeBridgeProjectedThing,
        out_reference: *mut *mut TypeBridgeProjectedReference,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_field_value_at(
        thing: *const TypeBridgeProjectedThing,
        field: *const TypeBridgeProjectedTokenV1,
        index: usize,
        out_value: *mut *mut TypeBridgeProjectedValue,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_role_count(
        thing: *const TypeBridgeProjectedThing,
        role: *const TypeBridgeProjectedTokenV1,
        out_count: *mut usize,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_role_reference_at(
        thing: *const TypeBridgeProjectedThing,
        role: *const TypeBridgeProjectedTokenV1,
        index: usize,
        out_reference: *mut *mut TypeBridgeProjectedReference,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_scalar_role_reference(
        thing: *const TypeBridgeProjectedThing,
        role: *const TypeBridgeProjectedTokenV1,
        out_reference: *mut *mut TypeBridgeProjectedReference,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_projected_thing_close(
        thing: *mut *mut TypeBridgeProjectedThing,
    ) -> TypeBridgeStatus;
}

struct Fixture {
    package: *mut TypeBridgeSchemaPackage,
    projection: RuntimeProjection,
    declared_identity: DeclaredIdentityFingerprint,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.package.is_null() {
            // SAFETY: this fixture owns the package slot.
            assert_eq!(
                unsafe { type_bridge_schema_package_close(&mut self.package) },
                TypeBridgeStatus::Ok,
            );
        }
    }
}

fn bytes(value: &[u8]) -> TypeBridgeByteView {
    TypeBridgeByteView {
        data: value.as_ptr(),
        length: value.len(),
    }
}

fn opaque_input(
    kind: u32,
    pointer: *const c_void,
    count: usize,
) -> TypeBridgeGeneratedOpaqueInputV1 {
    TypeBridgeGeneratedOpaqueInputV1 {
        struct_size: size_of::<TypeBridgeGeneratedOpaqueInputV1>() as u32,
        version: 1,
        kind,
        reserved0: 0,
        pointer,
        count,
        reserved: [0; 4],
    }
}

fn output_range(pointer: *mut c_void, length: usize) -> TypeBridgeGeneratedOutputRangeV1 {
    TypeBridgeGeneratedOutputRangeV1 {
        struct_size: size_of::<TypeBridgeGeneratedOutputRangeV1>() as u32,
        version: 1,
        pointer,
        length,
        reserved: [0; 4],
    }
}

fn text(value: &str) -> TypeBridgeByteView {
    bytes(value.as_bytes())
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type identity is valid")
}

fn field(owner: &TypeId, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(
        owner.clone(),
        AttributeId::new(attribute).expect("fixture attribute identity is valid"),
    )
    .expect("fixture owns identity is valid")
}

fn open_fixture(source: &str, prefix: &str, scope: &str) -> Fixture {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new(format!("{scope}.yaml")).expect("fixture document ID is valid"),
        source,
    )])
    .expect("fixture schema parses");
    let declared = normalize_documents(&documents).expect("fixture schema normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("fixture schema resolves");
    let capabilities: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("fixture scope is valid"),
        profile,
        capabilities,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("fixture authority builds");
    let emitter = CEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new(prefix).expect("fixture prefix is valid")),
        &emitter.generator_handlers(),
        &emitter.code_resources().expect("C resources hash"),
    )
    .expect("fixture projects");

    let authority_json = encode_schema_authority(&authority);
    let declared_json =
        encode_declared_schema(authority.declared_schema()).expect("schema encodes");
    let projection_json = to_canonical_json(&projection).expect("projection encodes");
    let semantic_json =
        to_canonical_json(projection.semantic_fingerprint()).expect("semantic fingerprint encodes");
    let binding_json = to_canonical_json(projection.projection_fingerprint())
        .expect("projection fingerprint encodes");
    let scope_bytes = authority.managed_scope().id().as_str().as_bytes();
    let profile_bytes = authority.semantic_profile().id().as_str().as_bytes();
    let descriptor = TypeBridgeSchemaPackageDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeSchemaPackageDescriptorV1>())
            .expect("descriptor size fits u32"),
        abi_major: 1,
        abi_minor: 1,
        schema_authority_json: bytes(&authority_json),
        declared_schema_json: bytes(&declared_json),
        runtime_projection_json: bytes(&projection_json),
        semantic_fingerprint_json: bytes(&semantic_json),
        binding_fingerprint_json: bytes(&binding_json),
        managed_scope: bytes(scope_bytes),
        semantic_profile: bytes(profile_bytes),
        reserved: [0; 4],
    };
    let mut package = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: descriptor inputs live for the call and outputs are writable.
    assert_eq!(
        unsafe { type_bridge_schema_package_open_v1(&descriptor, &mut package, &mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
    assert!(!package.is_null());
    assert!(diagnostics.is_null());
    Fixture {
        package,
        projection,
        declared_identity: authority
            .resolved_schema()
            .declared_identity_fingerprint()
            .clone(),
    }
}

fn token(
    projection: &RuntimeProjection,
    identity: ProjectedTokenIdentity,
) -> TypeBridgeProjectedTokenV1 {
    TypeBridgeProjectedTokenV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedTokenV1>())
            .expect("token size fits u32"),
        version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
        kind: identity.kind().as_u32(),
        ordinal: projection
            .projected_token_ordinal(&identity)
            .expect("fixture token is projected"),
        projection_digest: projection
            .projection_fingerprint()
            .as_fingerprint()
            .digest()
            .bytes(),
        reserved: [0; 4],
    }
}

fn model_token(
    projection: &RuntimeProjection,
    kind: TypeKind,
    label: &str,
) -> TypeBridgeProjectedTokenV1 {
    token(
        projection,
        ProjectedTokenIdentity::Model(type_id(kind, label)),
    )
}

fn field_token(
    projection: &RuntimeProjection,
    owner: &TypeId,
    attribute: &str,
) -> TypeBridgeProjectedTokenV1 {
    token(
        projection,
        ProjectedTokenIdentity::Field {
            owner: owner.clone(),
            field: field(owner, attribute),
        },
    )
}

fn role_token(
    projection: &RuntimeProjection,
    owner: &TypeId,
    label: &str,
) -> TypeBridgeProjectedTokenV1 {
    token(
        projection,
        ProjectedTokenIdentity::Role {
            owner: owner.clone(),
            role: RoleId::new(owner.label().as_str(), label).expect("fixture role is valid"),
        },
    )
}

type TextOpen = unsafe extern "C" fn(
    *const TypeBridgeSchemaPackage,
    *const TypeBridgeProjectedTokenV1,
    TypeBridgeByteView,
    *mut *mut TypeBridgeProjectedValue,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus;

fn open_text_value(
    fixture: &Fixture,
    token: &TypeBridgeProjectedTokenV1,
    value: &str,
    open: TextOpen,
) -> *mut TypeBridgeProjectedValue {
    let mut output = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: fixture/token/text inputs are live and outputs are writable.
    assert_eq!(
        unsafe {
            open(
                fixture.package,
                token,
                text(value),
                &mut output,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!output.is_null());
    assert!(diagnostics.is_null());
    output
}

fn field_input(
    field: &TypeBridgeProjectedTokenV1,
    values: &[*const TypeBridgeProjectedValue],
) -> TypeBridgeProjectedFieldInputV1 {
    TypeBridgeProjectedFieldInputV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedFieldInputV1>()).unwrap(),
        version: 1,
        field,
        values: values.as_ptr(),
        value_count: values.len(),
        reserved: [0; 4],
    }
}

fn role_input(
    role: &TypeBridgeProjectedTokenV1,
    references: &[*const TypeBridgeProjectedReference],
) -> TypeBridgeProjectedRoleInputV1 {
    TypeBridgeProjectedRoleInputV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedRoleInputV1>()).unwrap(),
        version: 1,
        role,
        references: references.as_ptr(),
        reference_count: references.len(),
        reserved: [0; 4],
    }
}

fn close_value(value: &mut *mut TypeBridgeProjectedValue) {
    // SAFETY: caller owns this exact value slot.
    assert_eq!(
        unsafe { type_bridge_projected_value_close(value) },
        TypeBridgeStatus::Ok,
    );
    assert!(value.is_null());
}

fn copied(view: TypeBridgeByteView) -> Vec<u8> {
    if view.length == 0 {
        Vec::new()
    } else {
        // SAFETY: test only calls this while the originating handle is live.
        unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec()
    }
}

#[test]
fn all_nine_domains_person_create_reference_and_membership_are_package_branded() {
    let mut fixture = open_fixture(SOURCE, "abi", "projected-model-abi");
    let person = type_id(TypeKind::Entity, "person");
    let membership = type_id(TypeKind::Relation, "membership");
    let attribute_names = [
        "identifier",
        "measure-long",
        "measure-double",
        "enabled",
        "born-on",
        "observed-at",
        "zoned-at",
        "balance",
        "elapsed",
    ];
    let attribute_tokens =
        attribute_names.map(|name| model_token(&fixture.projection, TypeKind::Attribute, name));
    let person_field_tokens =
        attribute_names.map(|name| field_token(&fixture.projection, &person, name));

    let mut values = Vec::new();
    values.push(open_text_value(
        &fixture,
        &attribute_tokens[0],
        "ada",
        type_bridge_projected_value_string_open,
    ));
    let mut long = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_long_open(
                fixture.package,
                &attribute_tokens[1],
                -10,
                &mut long,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    values.push(long);
    let expected_bits = (-0.0_f64).to_bits();
    let mut double = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_double_open(
                fixture.package,
                &attribute_tokens[2],
                expected_bits,
                &mut double,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    values.push(double);
    let mut boolean = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_boolean_open(
                fixture.package,
                &attribute_tokens[3],
                1,
                &mut boolean,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    values.push(boolean);
    for (index, value, open) in [
        (
            4,
            "2026-08-11",
            type_bridge_projected_value_date_open as TextOpen,
        ),
        (
            5,
            "2026-08-11T07:30:00.5",
            type_bridge_projected_value_datetime_open as TextOpen,
        ),
        (
            6,
            "2026-08-11T07:30:00Z",
            type_bridge_projected_value_datetime_tz_open as TextOpen,
        ),
        (
            7,
            "12.5",
            type_bridge_projected_value_decimal_open as TextOpen,
        ),
        (
            8,
            "P1M2DT3.000000004S",
            type_bridge_projected_value_duration_open as TextOpen,
        ),
    ] {
        values.push(open_text_value(
            &fixture,
            &attribute_tokens[index],
            value,
            open,
        ));
    }
    assert!(diagnostics.is_null());

    let value_arrays = values
        .iter()
        .map(|value| [*value as *const TypeBridgeProjectedValue])
        .collect::<Vec<_>>();
    let fields = person_field_tokens
        .iter()
        .zip(&value_arrays)
        .map(|(field, values)| field_input(field, values))
        .collect::<Vec<_>>();
    let person_model = model_token(&fixture.projection, TypeKind::Entity, "person");
    let person_descriptor = TypeBridgeProjectedCreateDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedCreateDescriptorV1>()).unwrap(),
        version: 1,
        model: &person_model,
        fields: fields.as_ptr(),
        field_count: fields.len(),
        roles: ptr::null(),
        role_count: 0,
        reserved: [0; 4],
    };
    let mut person_create = ptr::null_mut();
    // SAFETY: descriptor graph and outputs remain live for the call.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_open_v1(
                fixture.package,
                &person_descriptor,
                &mut person_create,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );

    let person_thing_descriptor = TypeBridgeProjectedThingDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedThingDescriptorV1>()).unwrap(),
        version: 1,
        model: &person_model,
        iid: text("0xaaa"),
        fields: fields.as_ptr(),
        field_count: fields.len(),
        roles: ptr::null(),
        role_count: 0,
        reserved: [0; 4],
    };
    let mut person_thing = ptr::null_mut();
    // SAFETY: descriptor graph and outputs remain live for the call.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_open_v1(
                fixture.package,
                &person_thing_descriptor,
                &mut person_thing,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );

    let iid = "0xabc";
    let key_values = [values[0] as *const TypeBridgeProjectedValue];
    let keys = [field_input(&person_field_tokens[0], &key_values)];
    let reference_descriptor = TypeBridgeProjectedReferenceDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedReferenceDescriptorV1>()).unwrap(),
        version: 1,
        model: &person_model,
        iid: text(iid),
        keys: keys.as_ptr(),
        key_count: keys.len(),
        reserved: [0; 4],
    };
    let mut reference = ptr::null_mut();
    // SAFETY: descriptor and outputs remain live for the call.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_open_v1(
                fixture.package,
                &reference_descriptor,
                &mut reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let iid_only_descriptor = TypeBridgeProjectedReferenceDescriptorV1 {
        keys: ptr::null(),
        key_count: 0,
        ..reference_descriptor
    };
    let mut iid_only_reference = ptr::null_mut();
    // SAFETY: descriptor and outputs remain live for the call.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_open_v1(
                fixture.package,
                &iid_only_descriptor,
                &mut iid_only_reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );

    let membership_model = model_token(&fixture.projection, TypeKind::Relation, "membership");
    let member_role = role_token(&fixture.projection, &membership, "member");
    let role_references = [reference as *const TypeBridgeProjectedReference];
    let roles = [role_input(&member_role, &role_references)];
    let membership_descriptor = TypeBridgeProjectedCreateDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedCreateDescriptorV1>()).unwrap(),
        version: 1,
        model: &membership_model,
        fields: ptr::null(),
        field_count: 0,
        roles: roles.as_ptr(),
        role_count: roles.len(),
        reserved: [0; 4],
    };
    let mut membership_create = ptr::null_mut();
    // SAFETY: descriptor graph and outputs remain live for the call.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_open_v1(
                fixture.package,
                &membership_descriptor,
                &mut membership_create,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );

    let thing_descriptor = TypeBridgeProjectedThingDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedThingDescriptorV1>()).unwrap(),
        version: 1,
        model: &membership_model,
        iid: text("0xdef"),
        fields: ptr::null(),
        field_count: 0,
        roles: roles.as_ptr(),
        role_count: roles.len(),
        reserved: [0; 4],
    };
    let mut thing = ptr::null_mut();
    // SAFETY: descriptor graph and outputs remain live for the call.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_open_v1(
                fixture.package,
                &thing_descriptor,
                &mut thing,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());

    let mut cancellation = ptr::null_mut();
    // SAFETY: the owner slot is writable and initially null.
    assert_eq!(
        unsafe { type_bridge_cancellation_open(&mut cancellation) },
        TypeBridgeStatus::Ok,
    );
    let options = TypeBridgeProjectedCodecOptionsV1 {
        struct_size: size_of::<TypeBridgeProjectedCodecOptionsV1>() as u64,
        version: 1,
        flags: 0,
        timeout_milliseconds: 0,
        max_input_bytes: 16 * 1024 * 1024,
        max_output_bytes: 16 * 1024 * 1024,
        max_depth: 64,
        max_records: 1,
        max_members: 65_536,
        cancellation,
    };
    // An output inside the complete options object is rejected before initialization.
    // SAFETY: the hostile output aliases the live options object and is rejected by preflight.
    assert_eq!(
        unsafe {
            type_bridge_canonical_record_encode_create_v1(
                person_create,
                &options,
                (&options as *const TypeBridgeProjectedCodecOptionsV1)
                    .cast_mut()
                    .cast(),
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());
    // SAFETY: the live cancellation handle accepts one sticky request.
    assert_eq!(
        unsafe { type_bridge_cancellation_request(cancellation) },
        TypeBridgeStatus::Ok,
    );
    let mut cancelled_bytes = ptr::null_mut();
    // SAFETY: all inputs and distinct outputs remain live for the controlled call.
    let status = unsafe {
        type_bridge_canonical_record_encode_create_v1(
            person_create,
            &options,
            &mut cancelled_bytes,
            &mut diagnostics,
        )
    };
    assert!(cancelled_bytes.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::Cancelled,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Cancelled,
        "provider_cancelled",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: the exact cancellation owner slot is live and uniquely owned.
    assert_eq!(
        unsafe { type_bridge_cancellation_close(&mut cancellation) },
        TypeBridgeStatus::Ok,
    );

    // An output slot inside the live projected input is rejected before publication.
    // SAFETY: the deliberately hostile output aliases a live input object; preflight rejects it
    // before writing either output.
    assert_eq!(
        unsafe {
            type_bridge_canonical_record_encode_create_v1(
                person_create,
                ptr::null(),
                person_create.cast(),
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());

    let mut record_bytes = ptr::null_mut();
    // SAFETY: the projected create is live and both outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_canonical_record_encode_create_v1(
                person_create,
                ptr::null(),
                &mut record_bytes,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut record_view = text("");
    // SAFETY: owned bytes and the borrowed-view output remain live.
    assert_eq!(
        unsafe { type_bridge_canonical_bytes_view(record_bytes, &mut record_view) },
        TypeBridgeStatus::Ok,
    );
    let expected_record = copied(record_view);
    // A view output placed inside the retained canonical buffer is rejected without mutation.
    // SAFETY: the deliberately hostile output aliases borrowed immutable bytes; preflight rejects
    // it before attempting an unaligned view write.
    assert_eq!(
        unsafe {
            type_bridge_canonical_bytes_view(record_bytes, record_view.data.cast_mut().cast())
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(copied(record_view), expected_record);
    let mut archive_cancellation = ptr::null_mut();
    // SAFETY: the owner slot is writable and initially null.
    assert_eq!(
        unsafe { type_bridge_cancellation_open(&mut archive_cancellation) },
        TypeBridgeStatus::Ok,
    );
    let archive_options = TypeBridgeProjectedCodecOptionsV1 {
        cancellation: archive_cancellation,
        max_output_bytes: 32 * 1024 * 1024,
        max_records: 4_096,
        ..options
    };
    let mut cancelled_builder = ptr::null_mut();
    // SAFETY: package, options, and distinct outputs remain live for control capture.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_builder_open_v1(
                fixture.package,
                &archive_options,
                &mut cancelled_builder,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: builder and borrowed record bytes remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_builder_append_record_v1(
                cancelled_builder,
                record_view,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: the live cancellation handle accepts one sticky request.
    assert_eq!(
        unsafe { type_bridge_cancellation_request(archive_cancellation) },
        TypeBridgeStatus::Ok,
    );
    let mut cancelled_archive_bytes = ptr::null_mut();
    // SAFETY: builder ownership and result slots remain live and distinct.
    let status = unsafe {
        type_bridge_canonical_archive_builder_finish_v1(
            &mut cancelled_builder,
            &mut cancelled_archive_bytes,
            &mut diagnostics,
        )
    };
    assert!(!cancelled_builder.is_null());
    assert!(cancelled_archive_bytes.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::Cancelled,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Cancelled,
        "provider_cancelled",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: closing the caller owner does not invalidate the captured builder control.
    assert_eq!(
        unsafe { type_bridge_cancellation_close(&mut archive_cancellation) },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: the retained failed builder remains independently owned and closeable.
    assert_eq!(
        unsafe { type_bridge_canonical_archive_builder_close(&mut cancelled_builder) },
        TypeBridgeStatus::Ok,
    );

    let mut archive_builder = ptr::null_mut();
    // SAFETY: package and outputs are live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_builder_open_v1(
                fixture.package,
                ptr::null(),
                &mut archive_builder,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: builder and borrowed record bytes remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_builder_append_record_v1(
                archive_builder,
                record_view,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut archive_bytes = ptr::null_mut();
    // SAFETY: builder ownership slot and outputs are live and distinct.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_builder_finish_v1(
                &mut archive_builder,
                &mut archive_bytes,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(archive_builder.is_null());
    let mut archive_view = text("");
    // SAFETY: owned archive bytes and view output remain live.
    assert_eq!(
        unsafe { type_bridge_canonical_bytes_view(archive_bytes, &mut archive_view) },
        TypeBridgeStatus::Ok,
    );
    let mut archive = ptr::null_mut();
    // SAFETY: package, archive bytes, and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_open_v1(
                fixture.package,
                archive_view,
                ptr::null(),
                &mut archive,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut archive_count = 0;
    // SAFETY: archive and count output remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_count(archive, &mut archive_count, &mut diagnostics)
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(archive_count, 1);
    let mut recovered = ptr::null_mut();
    // SAFETY: archive and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_archive_record_at(archive, 0, &mut recovered, &mut diagnostics)
        },
        TypeBridgeStatus::Ok,
    );
    let mut recovered_view = text("");
    // SAFETY: recovered bytes and output remain live.
    assert_eq!(
        unsafe { type_bridge_canonical_bytes_view(recovered, &mut recovered_view) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(recovered_view), expected_record);
    let limited_options = TypeBridgeProjectedCodecOptionsV1 {
        struct_size: size_of::<TypeBridgeProjectedCodecOptionsV1>() as u64,
        version: 1,
        flags: 0,
        timeout_milliseconds: 0,
        max_input_bytes: (recovered_view.length - 1) as u64,
        max_output_bytes: 16 * 1024 * 1024,
        max_depth: 64,
        max_records: 1,
        max_members: 65_536,
        cancellation: ptr::null(),
    };
    let mut limited_create = ptr::null_mut();
    // SAFETY: complete inputs and distinct outputs remain live for the limited decode.
    let status = unsafe {
        type_bridge_canonical_record_decode_create_v1(
            fixture.package,
            recovered_view,
            &person_model,
            &limited_options,
            &mut limited_create,
            &mut diagnostics,
        )
    };
    assert!(limited_create.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::ResourceLimit,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::ResourceLimit,
        "c_canonical_input_limit",
    );
    diagnostics = ptr::null_mut();
    // A decoded-owner output placed inside its canonical input is rejected without mutation.
    // SAFETY: the deliberately hostile owner slot aliases borrowed canonical bytes; preflight
    // rejects it before initializing or publishing an owner.
    assert_eq!(
        unsafe {
            type_bridge_canonical_record_decode_create_v1(
                fixture.package,
                recovered_view,
                &person_model,
                ptr::null(),
                recovered_view.data.cast_mut().cast(),
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());
    assert_eq!(copied(recovered_view), expected_record);
    let mut decoded_create = ptr::null_mut();
    // SAFETY: package, canonical bytes, exact generated token, and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_record_decode_create_v1(
                fixture.package,
                recovered_view,
                &person_model,
                ptr::null(),
                &mut decoded_create,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!decoded_create.is_null());
    // SAFETY: the decoded create owns one exact handle.
    assert_eq!(
        unsafe { type_bridge_projected_create_close(&mut decoded_create) },
        TypeBridgeStatus::Ok,
    );
    // A foreign generated target is rejected before any decoded handle is published.
    let status = unsafe {
        type_bridge_canonical_record_decode_create_v1(
            fixture.package,
            recovered_view,
            &membership_model,
            ptr::null(),
            &mut decoded_create,
            &mut diagnostics,
        )
    };
    assert!(decoded_create.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_canonical_record_type_mismatch",
    );
    diagnostics = ptr::null_mut();

    let installed =
        type_bridge_orm::InstalledRuntimeProjection::try_new(fixture.projection.clone())
            .expect("fixture projection installs")
            .with_declared_schema_identity(fixture.declared_identity.clone());
    let score_card_id = TypeId::new(TypeKind::Struct, "score-card").unwrap();
    let score_card = type_bridge_orm::ProjectedStructValue::try_new(
        &installed,
        score_card_id,
        vec![Some(CanonicalValue::Long(42)), None],
    )
    .expect("fixture struct projects");
    let score_card_bytes = type_bridge_orm::record_from_struct(&installed, &score_card)
        .expect("fixture struct records")
        .encode()
        .expect("fixture struct record encodes");
    let score_card_token = token(
        &fixture.projection,
        ProjectedTokenIdentity::Struct(StructId::new("score-card").unwrap()),
    );
    let mut decoded_struct = ptr::null_mut();
    // SAFETY: package, canonical record, exact struct token, and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_canonical_record_decode_struct_v1(
                fixture.package,
                bytes(&score_card_bytes),
                &score_card_token,
                ptr::null(),
                &mut decoded_struct,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut score_member = ptr::null_mut();
    // SAFETY: decoded struct, exact generated token, and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_struct_member_at_v1(
                decoded_struct,
                &score_card_token,
                0,
                &mut score_member,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut score = 0;
    // SAFETY: member handle and scalar output remain live.
    assert_eq!(
        unsafe { type_bridge_projected_struct_member_long(score_member, &mut score) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(score, 42);
    // The absent optional member is represented by a successful null owner.
    let mut absent_member = ptr::null_mut();
    // SAFETY: decoded struct, exact generated token, and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_struct_member_at_v1(
                decoded_struct,
                &score_card_token,
                1,
                &mut absent_member,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(absent_member.is_null());
    // SAFETY: each slot owns one exact independent handle.
    assert_eq!(
        unsafe { type_bridge_projected_struct_member_close(&mut score_member) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_struct_close(&mut decoded_struct) },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: each slot owns one exact ABI 1.6 handle.
    assert_eq!(
        unsafe { type_bridge_canonical_bytes_close(&mut recovered) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_canonical_archive_close(&mut archive) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_canonical_bytes_close(&mut archive_bytes) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_canonical_bytes_close(&mut record_bytes) },
        TypeBridgeStatus::Ok,
    );

    // Projected handles retain immutable package state after the public package closes.
    // SAFETY: fixture owns this exact package slot.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(&mut fixture.package) },
        TypeBridgeStatus::Ok,
    );

    // The generated-only nominal fences retain enough package identity to
    // validate an exact model even after the public package handle is closed.
    // SAFETY: retained handles/tokens are live and diagnostics is writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_validate_model(
                person_thing,
                &person_model,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());
    // SAFETY: retained handles/tokens are live and diagnostics is writable.
    let status = unsafe {
        type_bridge_projected_thing_validate_model(
            person_thing,
            &membership_model,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_thing_model_mismatch",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and diagnostics is writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_validate_model(
                reference,
                &person_model,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());
    // SAFETY: retained handles/tokens are live and diagnostics is writable.
    let status = unsafe {
        type_bridge_projected_reference_validate_model(
            reference,
            &membership_model,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_reference_model_mismatch",
    );
    diagnostics = ptr::null_mut();

    // The union discriminator and clone retain exact nominal, package, IID,
    // key, and origin evidence without exposing Rust layout.
    let mut ordinal = u32::MAX;
    // SAFETY: retained reference and independent outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_model_ordinal(reference, &mut ordinal, &mut diagnostics)
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(ordinal, person_model.ordinal);
    assert!(diagnostics.is_null());
    let mut cloned_reference = ptr::dangling_mut();
    // SAFETY: retained reference and independent outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_clone(
                reference,
                &mut cloned_reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!cloned_reference.is_null());
    let mut cloned_iid = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: the cloned reference is live and the borrowed-view output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_reference_iid(cloned_reference, &mut cloned_iid) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(cloned_iid), b"0xabc");
    let mut cloned_key = ptr::null_mut();
    // SAFETY: clone/key token are live and independent outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_key(
                cloned_reference,
                &person_field_tokens[0],
                &mut cloned_key,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!cloned_key.is_null());
    close_value(&mut cloned_key);
    // SAFETY: this slot uniquely owns the independent clone.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut cloned_reference) },
        TypeBridgeStatus::Ok,
    );
    assert!(!reference.is_null());

    let team = type_id(TypeKind::Relation, "team");
    let participant_role = role_token(&fixture.projection, &team, "participant");
    let container = type_id(TypeKind::Relation, "container");
    let item_role = role_token(&fixture.projection, &container, "item");
    // The same exact player may validate for a different role under the
    // domain-only policy when that role admits it.
    for accepted_role in [&member_role, &participant_role] {
        // SAFETY: reference/token are retained and diagnostics is writable.
        assert_eq!(
            unsafe {
                type_bridge_projected_reference_validate_role(
                    reference,
                    accepted_role,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok,
        );
        assert!(diagnostics.is_null());
    }
    // A zero-player role rejects every defensive nonnull union cast.
    // SAFETY: reference/token are retained and diagnostics is writable.
    let status = unsafe {
        type_bridge_projected_reference_validate_role(reference, &item_role, &mut diagnostics)
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_reference_role_player_mismatch",
    );
    diagnostics = ptr::null_mut();
    let foreign = open_fixture(SOURCE, "foreign_role", "projected-role-foreign");
    let foreign_membership = type_id(TypeKind::Relation, "membership");
    let foreign_member_role = role_token(&foreign.projection, &foreign_membership, "member");
    // SAFETY: both handles are live; the retained package must reject the foreign token.
    let status = unsafe {
        type_bridge_projected_reference_validate_role(
            reference,
            &foreign_member_role,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::ExecutionFailed,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Integrity,
        "generated_token_package_mismatch",
    );
    diagnostics = ptr::null_mut();
    let mut forged_role = member_role;
    forged_role.projection_digest[0] ^= 0xff;
    // SAFETY: reference/forged token storage remain live and diagnostics is writable.
    let status = unsafe {
        type_bridge_projected_reference_validate_role(reference, &forged_role, &mut diagnostics)
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::ExecutionFailed,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Integrity,
        "generated_token_package_mismatch",
    );
    diagnostics = ptr::null_mut();

    let mut scalar_role_reference = ptr::dangling_mut();
    // SAFETY: the hydrated membership and role token are retained and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_scalar_role_reference(
                thing,
                &member_role,
                &mut scalar_role_reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!scalar_role_reference.is_null());
    // SAFETY: this slot uniquely owns the scalar-role clone.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut scalar_role_reference) },
        TypeBridgeStatus::Ok,
    );

    // Every helper performs full-size input/output alias preflight with zero writes.
    let aliased_reference_output = unsafe {
        reference
            .cast::<u8>()
            .add(1)
            .cast::<*mut TypeBridgeProjectedReference>()
    };
    diagnostics = ptr::dangling_mut();
    // SAFETY: the hostile interior output must be rejected without a write.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_clone(
                reference,
                aliased_reference_output,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());
    let aliased_ordinal = unsafe { reference.cast::<u8>().add(1).cast::<u32>() };
    // SAFETY: the hostile interior output must be rejected without a write.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_model_ordinal(
                reference,
                aliased_ordinal,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());
    let aliased_role_diagnostics = unsafe {
        (&member_role as *const TypeBridgeProjectedTokenV1)
            .cast::<u8>()
            .add(1)
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>()
    };
    // SAFETY: the hostile token-interior output must be rejected without a write.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_validate_role(
                reference,
                &member_role,
                aliased_role_diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    let aliased_scalar_output = unsafe {
        thing
            .cast::<u8>()
            .add(1)
            .cast::<*mut TypeBridgeProjectedReference>()
    };
    // SAFETY: the hostile thing-interior output must be rejected without a write.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_scalar_role_reference(
                thing,
                &member_role,
                aliased_scalar_output,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());

    // A null peer clears only the independent diagnostics slot before rejection.
    // SAFETY: the live reference is disjoint from the one nonnull output slot.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_clone(reference, ptr::null_mut(), &mut diagnostics)
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());
    diagnostics = ptr::dangling_mut();
    // SAFETY: the live reference is disjoint from the one nonnull output slot.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_model_ordinal(
                reference,
                ptr::null_mut(),
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());
    diagnostics = ptr::dangling_mut();
    // SAFETY: the live thing/token are disjoint from the one nonnull output slot.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_scalar_role_reference(
                thing,
                &member_role,
                ptr::null_mut(),
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());

    // Reuse after every hostile call proves the original handles and token survived.
    // SAFETY: retained reference/token and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_model_ordinal(reference, &mut ordinal, &mut diagnostics)
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(ordinal, person_model.ordinal);

    let alias_model = model_token(
        &fixture.projection,
        TypeKind::Attribute,
        "secondary-identifier",
    );
    // SAFETY: retained handles/tokens are live and diagnostics is writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_validate_model(
                values[0],
                &attribute_tokens[0],
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());
    // A distinct string attribute has the same scalar domain, so only the
    // exact generated model token can reject this otherwise plausible cast.
    // SAFETY: retained handles/tokens are live and diagnostics is writable.
    let status = unsafe {
        type_bridge_projected_value_validate_model(values[0], &alias_model, &mut diagnostics)
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_value_model_mismatch",
    );
    diagnostics = ptr::null_mut();

    // Hostile interior output aliases are rejected without corrupting the live
    // thing/token, proven by immediately reusing both in a successful call.
    // SAFETY: the deliberately aliased slot is never written by the read-only preflight.
    let aliased_diagnostics = unsafe {
        person_thing
            .cast::<u8>()
            .add(1)
            .cast::<*mut TypeBridgeExecutionDiagnostics>()
    };
    // SAFETY: inputs are live; the hostile output must be rejected before access.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_validate_model(
                person_thing,
                &person_model,
                aliased_diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    // SAFETY: the preceding rejection preserved the thing and model token.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_validate_model(
                person_thing,
                &person_model,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());

    // SAFETY: the deliberately aliased slot is never written by the read-only preflight.
    let aliased_value_diagnostics = unsafe {
        values[0]
            .cast::<u8>()
            .add(1)
            .cast::<*mut TypeBridgeExecutionDiagnostics>()
    };
    // SAFETY: inputs are live; the hostile output must be rejected before access.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_validate_model(
                values[0],
                &attribute_tokens[0],
                aliased_value_diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    // SAFETY: the preceding rejection preserved the scalar and model token.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_validate_model(
                values[0],
                &attribute_tokens[0],
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());

    let mut actual_long = 0;
    // SAFETY: the retained value is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_long(values[1], &mut actual_long) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(actual_long, -10);
    let mut actual_bits = 0;
    // SAFETY: the retained value is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_double_bits(values[2], &mut actual_bits) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(actual_bits, expected_bits);
    let mut actual_boolean = 0;
    // SAFETY: the retained value is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_boolean(values[3], &mut actual_boolean) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(actual_boolean, 1);
    let mut kind = TypeBridgeProjectedValueKind::Long;
    // SAFETY: the retained value is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_kind(values[8], &mut kind) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(kind, TypeBridgeProjectedValueKind::Duration);
    let mut actual_text = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: the retained value is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_text(values[8], &mut actual_text) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(actual_text), b"P1M2DT3.000000004S");

    let mut count = 0;
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_field_count(
                person_create,
                &person_field_tokens[0],
                &mut count,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(count, 1);
    let mut aliased_count = usize::MAX;
    let count_slot = &mut aliased_count as *mut usize;
    let count_diagnostics_slot = count_slot.cast::<*mut TypeBridgeExecutionDiagnostics>();
    // SAFETY: deliberately aliased writable slots exercise count-output rejection.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_role_count(
                membership_create,
                &member_role,
                count_slot,
                count_diagnostics_slot,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(aliased_count, usize::MAX);

    let mut extracted_value = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_field_value_at(
                person_create,
                &person_field_tokens[0],
                0,
                &mut extracted_value,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut extracted_text = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: the cloned scalar handle is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_text(extracted_value, &mut extracted_text) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(extracted_text), b"ada");
    close_value(&mut extracted_value);

    // Ordinary sequence bounds failures carry one stable structured diagnostic.
    // SAFETY: retained handles/tokens are live and outputs are writable.
    let status = unsafe {
        type_bridge_projected_create_field_value_at(
            person_create,
            &person_field_tokens[0],
            1,
            &mut extracted_value,
            &mut diagnostics,
        )
    };
    assert!(extracted_value.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_index_out_of_bounds",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and outputs are writable.
    let status = unsafe {
        type_bridge_projected_thing_field_value_at(
            person_thing,
            &person_field_tokens[0],
            1,
            &mut extracted_value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_index_out_of_bounds",
    );
    diagnostics = ptr::null_mut();

    // The reference carries copied key evidence and returns it as a fresh value handle.
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_key(
                reference,
                &person_field_tokens[0],
                &mut extracted_value,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    close_value(&mut extracted_value);
    // An IID-only reference reports absent selected key evidence structurally.
    // SAFETY: retained handles/tokens are live and outputs are writable.
    let status = unsafe {
        type_bridge_projected_reference_key(
            iid_only_reference,
            &person_field_tokens[0],
            &mut extracted_value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_reference_key_missing",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_role_count(
                membership_create,
                &member_role,
                &mut count,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(count, 1);
    let mut extracted_reference = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and outputs are writable.
    let status = unsafe {
        type_bridge_projected_create_role_reference_at(
            membership_create,
            &member_role,
            1,
            &mut extracted_reference,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_index_out_of_bounds",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_create_role_reference_at(
                membership_create,
                &member_role,
                0,
                &mut extracted_reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut extracted_iid = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: the cloned reference is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_reference_iid(extracted_reference, &mut extracted_iid) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(extracted_iid), iid.as_bytes());
    // SAFETY: this exact reference family owns the cloned slot.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut extracted_reference) },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_role_count(
                thing,
                &member_role,
                &mut count,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(count, 1);
    // SAFETY: retained handles/tokens are live and outputs are writable.
    let status = unsafe {
        type_bridge_projected_thing_role_reference_at(
            thing,
            &member_role,
            1,
            &mut extracted_reference,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_index_out_of_bounds",
    );
    diagnostics = ptr::null_mut();
    // SAFETY: retained handles/tokens are live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_role_reference_at(
                thing,
                &member_role,
                0,
                &mut extracted_reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: this exact reference family owns the cloned slot.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut extracted_reference) },
        TypeBridgeStatus::Ok,
    );
    let mut actual_iid = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: retained handle is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_reference_iid(reference, &mut actual_iid) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(actual_iid), iid.as_bytes());
    // SAFETY: retained handle is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_thing_iid(thing, &mut actual_iid) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(actual_iid), b"0xdef");

    let mut aliased_reference = NonNull::<TypeBridgeProjectedReference>::dangling().as_ptr();
    let reference_slot = &mut aliased_reference as *mut *mut TypeBridgeProjectedReference;
    let reference_diagnostics_slot = reference_slot.cast::<*mut TypeBridgeExecutionDiagnostics>();
    // SAFETY: deliberately aliased writable slots exercise pre-dispatch rejection.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_reference(
                person_thing,
                reference_slot,
                reference_diagnostics_slot,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(
        aliased_reference,
        NonNull::<TypeBridgeProjectedReference>::dangling().as_ptr()
    );

    let mut derived_reference = ptr::null_mut();
    // A thing-derived reference preserves exact IID/key evidence and remains usable
    // through the thing's retained package state after the public package closes.
    // SAFETY: retained thing handle is live and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_reference(
                person_thing,
                &mut derived_reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: derived reference and output are live.
    assert_eq!(
        unsafe { type_bridge_projected_reference_iid(derived_reference, &mut actual_iid) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(actual_iid), b"0xaaa");
    // SAFETY: derived reference retains the exact projected key field.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_key(
                derived_reference,
                &person_field_tokens[0],
                &mut extracted_value,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    close_value(&mut extracted_value);
    let foreign_source = SOURCE.replace(
        "elapsed: { value: duration }",
        "elapsed: { value: duration }\n  extra: { value: string }",
    );
    let foreign = open_fixture(
        &foreign_source,
        "derived_foreign",
        "projected-derived-reference-foreign",
    );
    let foreign_membership = type_id(TypeKind::Relation, "membership");
    let foreign_membership_model =
        model_token(&foreign.projection, TypeKind::Relation, "membership");
    let foreign_member_role = role_token(&foreign.projection, &foreign_membership, "member");
    let derived_references = [derived_reference as *const TypeBridgeProjectedReference];
    let foreign_roles = [role_input(&foreign_member_role, &derived_references)];
    let foreign_descriptor = TypeBridgeProjectedCreateDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedCreateDescriptorV1>()).unwrap(),
        version: 1,
        model: &foreign_membership_model,
        fields: ptr::null(),
        field_count: 0,
        roles: foreign_roles.as_ptr(),
        role_count: foreign_roles.len(),
        reserved: [0; 4],
    };
    let mut foreign_create = ptr::null_mut();
    // A thing-derived reference cannot cross into a foreign projection package.
    // SAFETY: descriptor graph and outputs remain live for the call.
    let status = unsafe {
        type_bridge_projected_create_open_v1(
            foreign.package,
            &foreign_descriptor,
            &mut foreign_create,
            &mut diagnostics,
        )
    };
    assert!(foreign_create.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::ExecutionFailed,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Integrity,
        "generated_token_package_mismatch",
    );
    // SAFETY: this exact reference family owns the derived slot.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut derived_reference) },
        TypeBridgeStatus::Ok,
    );

    for value in &mut values {
        close_value(value);
    }
    // SAFETY: each exact handle family owns its slot.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut reference) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut iid_only_reference) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_create_close(&mut person_create) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_create_close(&mut membership_create) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_thing_close(&mut thing) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_thing_close(&mut person_thing) },
        TypeBridgeStatus::Ok,
    );
    // Idempotent null-slot close remains stable for every new family.
    // SAFETY: all slots are now null.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut reference) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_create_close(&mut person_create) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_create_close(&mut membership_create) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_thing_close(&mut thing) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        unsafe { type_bridge_projected_thing_close(&mut person_thing) },
        TypeBridgeStatus::Ok,
    );
}

#[test]
fn generated_preflight_rejects_opaque_interior_aliases_and_preserves_handles() {
    let fixture = open_fixture(SOURCE, "preflight", "generated-preflight-alias");
    let package_input = [opaque_input(
        GENERATED_INPUT_SCHEMA_PACKAGE,
        fixture.package.cast(),
        1,
    )];
    let package_interior = unsafe { fixture.package.cast::<u8>().add(1) }.cast::<c_void>();
    let outputs = [output_range(package_interior, 1)];
    assert_eq!(
        // SAFETY: descriptors and the deliberately interior allocated byte remain live.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                package_input.as_ptr(),
                package_input.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );

    // Reusing the package proves the read-only rejection did not corrupt its Arc.
    let string_token = model_token(&fixture.projection, TypeKind::Attribute, "identifier");
    let mut value = open_text_value(
        &fixture,
        &string_token,
        "still-live",
        type_bridge_projected_value_string_open,
    );
    let value_array = [value as *const TypeBridgeProjectedValue];
    let value_inputs = [opaque_input(
        GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
        value_array.as_ptr().cast(),
        value_array.len(),
    )];
    let value_interior = unsafe { value.cast::<u8>().add(1) }.cast::<c_void>();
    let outputs = [output_range(value_interior, 1)];
    assert_eq!(
        // SAFETY: the pointer array and its live pointee remain readable.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                value_inputs.as_ptr(),
                value_inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    let mut retained = bytes(&[]);
    // SAFETY: interior-alias rejection preserved the value allocation.
    assert_eq!(
        unsafe { type_bridge_projected_value_text(value, &mut retained) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(retained), b"still-live");
    close_value(&mut value);

    let mut caller_outputs = [0xa5_u8; 16];
    let overlapping_outputs = [
        output_range(caller_outputs.as_mut_ptr().cast(), 8),
        output_range(unsafe { caller_outputs.as_mut_ptr().add(1) }.cast(), 8),
    ];
    assert_eq!(
        // SAFETY: all output storage is allocated; helper promises zero writes.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                ptr::null(),
                0,
                overlapping_outputs.as_ptr(),
                overlapping_outputs.len(),
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(caller_outputs, [0xa5; 16]);
}

#[repr(C)]
struct OneCreateSequenceArgs {
    values: *const TypeBridgeGeneratedCreateHandleChunkV1,
}

fn create_graph_member(token: &TypeBridgeProjectedTokenV1) -> TypeBridgeGeneratedCreateMemberV1 {
    create_graph_member_kind(token, GENERATED_CREATE_MEMBER_VALUE_SEQUENCE)
}

fn create_graph_member_kind(
    token: &TypeBridgeProjectedTokenV1,
    kind: u32,
) -> TypeBridgeGeneratedCreateMemberV1 {
    TypeBridgeGeneratedCreateMemberV1 {
        struct_size: size_of::<TypeBridgeGeneratedCreateMemberV1>() as u32,
        version: GENERATED_CREATE_GRAPH_VERSION,
        kind,
        reserved0: 0,
        args_offset: std::mem::offset_of!(OneCreateSequenceArgs, values),
        token,
        reserved: [0; 4],
    }
}

fn repeated_handle_chunks(
    handle: *const c_void,
    count: usize,
) -> (
    Vec<Vec<*const c_void>>,
    Vec<TypeBridgeGeneratedCreateHandleChunkV1>,
) {
    let per_chunk = GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX / size_of::<*const c_void>();
    let arrays = (0..count)
        .step_by(per_chunk)
        .map(|start| vec![handle; (count - start).min(per_chunk)])
        .collect::<Vec<_>>();
    let mut chunks = arrays
        .iter()
        .map(|values| TypeBridgeGeneratedCreateHandleChunkV1 {
            struct_size: size_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
            version: GENERATED_CREATE_GRAPH_VERSION,
            values: values.as_ptr(),
            count: values.len(),
            next: ptr::null(),
            reserved: [0; 4],
        })
        .collect::<Vec<_>>();
    let base = chunks.as_ptr();
    for (index, chunk) in chunks.iter_mut().enumerate() {
        if index + 1 < arrays.len() {
            // The Vec has its final capacity and is not modified after linking.
            chunk.next = unsafe { base.add(index + 1) };
        }
    }
    (arrays, chunks)
}

fn create_graph(
    args: &OneCreateSequenceArgs,
    members: &[TypeBridgeGeneratedCreateMemberV1],
) -> TypeBridgeGeneratedCreateArgsGraphV1 {
    TypeBridgeGeneratedCreateArgsGraphV1 {
        struct_size: size_of::<TypeBridgeGeneratedCreateArgsGraphV1>() as u32,
        version: GENERATED_CREATE_GRAPH_VERSION,
        args: (args as *const OneCreateSequenceArgs).cast(),
        args_size: size_of::<OneCreateSequenceArgs>(),
        members: members.as_ptr(),
        member_count: members.len(),
        reserved: [0; 3],
    }
}

fn preflight_create_graph(
    graph: &TypeBridgeGeneratedCreateArgsGraphV1,
    outputs: &[TypeBridgeGeneratedOutputRangeV1],
) -> TypeBridgeStatus {
    let inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    // SAFETY: the caller retains the complete graph and output descriptors;
    // this helper performs no writes on any return path.
    unsafe {
        type_bridge_generated_opaque_alias_preflight_v1(
            inputs.as_ptr(),
            inputs.len(),
            outputs.as_ptr(),
            outputs.len(),
        )
    }
}

#[test]
fn generated_create_graph_rejects_null_cycle_and_nested_alias_without_writes() {
    let fixture = open_fixture(SOURCE, "graph", "generated-create-graph-hostile");
    let long_token = model_token(&fixture.projection, TypeKind::Attribute, "measure-long");
    let field = field_token(
        &fixture.projection,
        &type_id(TypeKind::Entity, "person"),
        "measure-long",
    );
    let mut value = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: all constructor inputs and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_long_open(
                fixture.package,
                &long_token,
                1,
                &mut value,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let members = [create_graph_member(&field)];
    let mut caller_output = [0xa5_u8; 16];
    let outputs = [output_range(
        caller_output.as_mut_ptr().cast(),
        caller_output.len(),
    )];

    let null_values = [ptr::null::<c_void>()];
    let null_chunk = TypeBridgeGeneratedCreateHandleChunkV1 {
        struct_size: size_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
        version: GENERATED_CREATE_GRAPH_VERSION,
        values: null_values.as_ptr(),
        count: null_values.len(),
        next: ptr::null(),
        reserved: [0; 4],
    };
    let args = OneCreateSequenceArgs {
        values: &null_chunk,
    };
    let graph = create_graph(&args, &members);
    let inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (&graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    // SAFETY: complete graph storage is readable; the null element is hostile input.
    assert_eq!(
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                inputs.as_ptr(),
                inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(caller_output, [0xa5; 16]);

    let handles = [value.cast::<c_void>().cast_const()];
    let mut cycle = TypeBridgeGeneratedCreateHandleChunkV1 {
        struct_size: size_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
        version: GENERATED_CREATE_GRAPH_VERSION,
        values: handles.as_ptr(),
        count: handles.len(),
        next: ptr::null(),
        reserved: [0; 4],
    };
    cycle.next = &cycle;
    let args = OneCreateSequenceArgs { values: &cycle };
    let graph = create_graph(&args, &members);
    let inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (&graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    // SAFETY: complete cyclic graph storage is readable and must be rejected finitely.
    assert_eq!(
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                inputs.as_ptr(),
                inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(caller_output, [0xa5; 16]);

    let chunk = TypeBridgeGeneratedCreateHandleChunkV1 {
        next: ptr::null(),
        ..cycle
    };
    let args = OneCreateSequenceArgs { values: &chunk };
    let graph = create_graph(&args, &members);
    let inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (&graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    let alias_outputs = [output_range(
        (&handles[0] as *const *const c_void).cast_mut().cast(),
        size_of::<*const c_void>(),
    )];
    let handle_before = handles[0];
    // SAFETY: the output deliberately aliases the nested pointer-array storage.
    assert_eq!(
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                inputs.as_ptr(),
                inputs.len(),
                alias_outputs.as_ptr(),
                alias_outputs.len(),
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(handles[0], handle_before);
    close_value(&mut value);
}

#[test]
fn generated_create_graph_rejects_malformed_layouts_offsets_and_direct_aliases_read_only() {
    let fixture = open_fixture(SOURCE, "graph_layout", "generated-create-graph-layout");
    let long_token = model_token(&fixture.projection, TypeKind::Attribute, "measure-long");
    let field = field_token(
        &fixture.projection,
        &type_id(TypeKind::Entity, "person"),
        "measure-long",
    );
    let mut value = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: all constructor inputs and outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_long_open(
                fixture.package,
                &long_token,
                1,
                &mut value,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let handles = [value.cast::<c_void>().cast_const()];
    let chunk = TypeBridgeGeneratedCreateHandleChunkV1 {
        struct_size: size_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
        version: GENERATED_CREATE_GRAPH_VERSION,
        values: handles.as_ptr(),
        count: handles.len(),
        next: ptr::null(),
        reserved: [0; 4],
    };
    let args = OneCreateSequenceArgs { values: &chunk };
    let member = create_graph_member(&field);
    let members = [member];
    let graph = create_graph(&args, &members);
    let mut sentinel = [0xa5_u8; 16];
    let outputs = [output_range(sentinel.as_mut_ptr().cast(), sentinel.len())];

    let mut malformed_graph = graph;
    malformed_graph.struct_size -= 1;
    assert_eq!(
        preflight_create_graph(&malformed_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(sentinel, [0xa5; 16]);
    malformed_graph = graph;
    malformed_graph.reserved[1] = 1;
    assert_eq!(
        preflight_create_graph(&malformed_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(sentinel, [0xa5; 16]);
    malformed_graph = graph;
    malformed_graph.members = ptr::null();
    assert_eq!(
        preflight_create_graph(&malformed_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(sentinel, [0xa5; 16]);

    let mut malformed_member = member;
    malformed_member.version += 1;
    let malformed_members = [malformed_member];
    let malformed_member_graph = create_graph(&args, &malformed_members);
    assert_eq!(
        preflight_create_graph(&malformed_member_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    malformed_member = member;
    malformed_member.kind = u32::MAX;
    let malformed_members = [malformed_member];
    let malformed_member_graph = create_graph(&args, &malformed_members);
    assert_eq!(
        preflight_create_graph(&malformed_member_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    malformed_member = member;
    malformed_member.args_offset = size_of::<OneCreateSequenceArgs>();
    let malformed_members = [malformed_member];
    let malformed_member_graph = create_graph(&args, &malformed_members);
    assert_eq!(
        preflight_create_graph(&malformed_member_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(sentinel, [0xa5; 16]);

    let mut malformed_chunk = chunk;
    malformed_chunk.struct_size -= 1;
    let malformed_args = OneCreateSequenceArgs {
        values: &malformed_chunk,
    };
    let malformed_chunk_graph = create_graph(&malformed_args, &members);
    assert_eq!(
        preflight_create_graph(&malformed_chunk_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    malformed_chunk = chunk;
    malformed_chunk.count = 0;
    let malformed_args = OneCreateSequenceArgs {
        values: &malformed_chunk,
    };
    let malformed_chunk_graph = create_graph(&malformed_args, &members);
    assert_eq!(
        preflight_create_graph(&malformed_chunk_graph, &outputs),
        TypeBridgeStatus::InvalidArgument,
    );
    malformed_chunk = chunk;
    malformed_chunk.count =
        GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX / size_of::<*const c_void>() + 1;
    let malformed_args = OneCreateSequenceArgs {
        values: &malformed_chunk,
    };
    let oversized_chunk_graph = create_graph(&malformed_args, &members);
    assert_eq!(
        preflight_create_graph(&oversized_chunk_graph, &outputs),
        TypeBridgeStatus::ResourceLimit,
    );
    assert_eq!(sentinel, [0xa5; 16]);

    let args_before = unsafe {
        std::slice::from_raw_parts(
            (&args as *const OneCreateSequenceArgs).cast::<u8>(),
            size_of::<OneCreateSequenceArgs>(),
        )
        .to_vec()
    };
    let args_output = [output_range(
        (&args as *const OneCreateSequenceArgs).cast_mut().cast(),
        size_of::<*const c_void>(),
    )];
    assert_eq!(
        preflight_create_graph(&graph, &args_output),
        TypeBridgeStatus::InvalidArgument,
    );
    let args_after = unsafe {
        std::slice::from_raw_parts(
            (&args as *const OneCreateSequenceArgs).cast::<u8>(),
            size_of::<OneCreateSequenceArgs>(),
        )
    };
    assert_eq!(args_after, args_before);

    let members_before = unsafe {
        std::slice::from_raw_parts(
            members.as_ptr().cast::<u8>(),
            size_of::<TypeBridgeGeneratedCreateMemberV1>(),
        )
        .to_vec()
    };
    let member_output = [output_range(
        members.as_ptr().cast_mut().cast(),
        size_of::<*const c_void>(),
    )];
    assert_eq!(
        preflight_create_graph(&graph, &member_output),
        TypeBridgeStatus::InvalidArgument,
    );
    let members_after = unsafe {
        std::slice::from_raw_parts(
            members.as_ptr().cast::<u8>(),
            size_of::<TypeBridgeGeneratedCreateMemberV1>(),
        )
    };
    assert_eq!(members_after, members_before);

    close_value(&mut value);
}

#[test]
fn generated_create_graph_matches_exact_value_and_reference_resource_boundaries() {
    let fixture = open_fixture(SOURCE, "graph_limit", "generated-create-graph-limits");
    let person = type_id(TypeKind::Entity, "person");
    let long_token = model_token(&fixture.projection, TypeKind::Attribute, "measure-long");
    let long_field = field_token(&fixture.projection, &person, "measure-long");
    let mut long = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: retained package/token and output slots satisfy the constructor contract.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_long_open(
                fixture.package,
                &long_token,
                1,
                &mut long,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );

    let (_value_arrays, mut value_chunks) =
        repeated_handle_chunks(long.cast::<c_void>().cast_const(), 65_535);
    let value_args = OneCreateSequenceArgs {
        values: value_chunks.as_ptr(),
    };
    let value_members = [create_graph_member(&long_field)];
    let value_graph = create_graph(&value_args, &value_members);
    let value_inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (&value_graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    let mut sentinel = usize::MAX;
    let outputs = [output_range(
        (&mut sentinel as *mut usize).cast(),
        size_of::<usize>(),
    )];
    value_chunks.last_mut().expect("nonempty chain").count -= 1;
    // One model + one member + 65,534 scalar values is the exact member ceiling.
    assert_eq!(
        // SAFETY: the complete bounded graph and output descriptor remain live.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                value_inputs.as_ptr(),
                value_inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(sentinel, usize::MAX);
    value_chunks.last_mut().expect("nonempty chain").count += 1;
    assert_eq!(
        // SAFETY: the same graph now contains exactly one excess scalar value.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                value_inputs.as_ptr(),
                value_inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::ResourceLimit,
    );
    assert_eq!(sentinel, usize::MAX);

    let person_model = model_token(&fixture.projection, TypeKind::Entity, "person");
    let reference_descriptor = TypeBridgeProjectedReferenceDescriptorV1 {
        struct_size: size_of::<TypeBridgeProjectedReferenceDescriptorV1>() as u32,
        version: 1,
        model: &person_model,
        iid: text("0xabc"),
        keys: ptr::null(),
        key_count: 0,
        reserved: [0; 4],
    };
    let mut reference = ptr::null_mut();
    // SAFETY: retained descriptor graph and output slots satisfy the constructor contract.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_open_v1(
                fixture.package,
                &reference_descriptor,
                &mut reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let membership = type_id(TypeKind::Relation, "membership");
    let member_role = role_token(&fixture.projection, &membership, "member");
    let (_reference_arrays, mut reference_chunks) =
        repeated_handle_chunks(reference.cast::<c_void>().cast_const(), 32_768);
    let reference_args = OneCreateSequenceArgs {
        values: reference_chunks.as_ptr(),
    };
    let reference_members = [create_graph_member_kind(
        &member_role,
        GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE,
    )];
    let reference_graph = create_graph(&reference_args, &reference_members);
    let reference_inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (&reference_graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    reference_chunks.last_mut().expect("nonempty chain").count -= 1;
    // One model + one role + 32,767 IID-only two-member references is exact.
    assert_eq!(
        // SAFETY: the complete bounded graph remains readable.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                reference_inputs.as_ptr(),
                reference_inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::Ok,
    );
    reference_chunks.last_mut().expect("nonempty chain").count += 1;
    assert_eq!(
        // SAFETY: one additional IID-only reference exceeds the shared ceiling.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                reference_inputs.as_ptr(),
                reference_inputs.len(),
                outputs.as_ptr(),
                outputs.len(),
            )
        },
        TypeBridgeStatus::ResourceLimit,
    );
    assert_eq!(sentinel, usize::MAX);

    // The resource-limit pass precedes nested heap alias fencing: an output
    // inside a repeated string's canonical text still receives zero writes.
    let string_token = model_token(&fixture.projection, TypeKind::Attribute, "identifier");
    let string_field = field_token(&fixture.projection, &person, "identifier");
    let mut string = open_text_value(
        &fixture,
        &string_token,
        "nested-live",
        type_bridge_projected_value_string_open,
    );
    let mut borrowed = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: retained string handle and writable view are live.
    assert_eq!(
        unsafe { type_bridge_projected_value_text(string, &mut borrowed) },
        TypeBridgeStatus::Ok,
    );
    let before = copied(borrowed);
    let (_string_arrays, string_chunks) =
        repeated_handle_chunks(string.cast::<c_void>().cast_const(), 65_535);
    let string_args = OneCreateSequenceArgs {
        values: string_chunks.as_ptr(),
    };
    let string_members = [create_graph_member(&string_field)];
    let string_graph = create_graph(&string_args, &string_members);
    let string_inputs = [opaque_input(
        GENERATED_INPUT_CREATE_ARGS_GRAPH,
        (&string_graph as *const TypeBridgeGeneratedCreateArgsGraphV1).cast(),
        1,
    )];
    let interior_output = [output_range(borrowed.data.cast_mut().cast(), 1)];
    assert_eq!(
        // SAFETY: the hostile output lies in live nested text and the graph is over budget.
        unsafe {
            type_bridge_generated_opaque_alias_preflight_v1(
                string_inputs.as_ptr(),
                string_inputs.len(),
                interior_output.as_ptr(),
                interior_output.len(),
            )
        },
        TypeBridgeStatus::ResourceLimit,
    );
    assert_eq!(copied(borrowed), before);

    close_value(&mut string);
    close_value(&mut long);
    // SAFETY: this slot uniquely owns the live projected reference.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut reference) },
        TypeBridgeStatus::Ok,
    );
}

#[test]
fn datetime_tz_fixed_and_named_values_round_trip_and_reopen() {
    let fixture = open_fixture(SOURCE, "abi", "projected-datetime-tz-round-trip");
    let token = model_token(&fixture.projection, TypeKind::Attribute, "zoned-at");

    for lexical in [
        "2026-01-11T07:30:00+01:00",
        "2026-01-11T07:30:00[Europe/Amsterdam]",
    ] {
        let mut value = open_text_value(
            &fixture,
            &token,
            lexical,
            type_bridge_projected_value_datetime_tz_open,
        );
        let mut view = bytes(&[]);
        // SAFETY: the value is live and the output view is writable.
        assert_eq!(
            unsafe { type_bridge_projected_value_text(value, &mut view) },
            TypeBridgeStatus::Ok,
        );
        let retained = String::from_utf8(copied(view)).expect("canonical text remains UTF-8");
        assert_eq!(retained, lexical);
        close_value(&mut value);

        let mut reopened = open_text_value(
            &fixture,
            &token,
            &retained,
            type_bridge_projected_value_datetime_tz_open,
        );
        // SAFETY: the reopened value is live and the output view is writable.
        assert_eq!(
            unsafe { type_bridge_projected_value_text(reopened, &mut view) },
            TypeBridgeStatus::Ok,
        );
        assert_eq!(copied(view), lexical.as_bytes());
        close_value(&mut reopened);
    }
}

#[test]
fn scalar_role_helper_returns_optional_null_and_constructors_reject_impossible_cardinality() {
    let fixture = open_fixture(SOURCE, "scalar_role", "projected-scalar-role");
    let optional = type_id(TypeKind::Relation, "optional-membership");
    let optional_model = model_token(
        &fixture.projection,
        TypeKind::Relation,
        "optional-membership",
    );
    let optional_role = role_token(&fixture.projection, &optional, "member");
    let optional_descriptor = TypeBridgeProjectedThingDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedThingDescriptorV1>()).unwrap(),
        version: 1,
        model: &optional_model,
        iid: text("0x40"),
        fields: ptr::null(),
        field_count: 0,
        roles: ptr::null(),
        role_count: 0,
        reserved: [0; 4],
    };
    let mut thing = ptr::dangling_mut();
    let mut diagnostics = ptr::dangling_mut();
    // SAFETY: descriptor inputs and independent output slots remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_open_v1(
                fixture.package,
                &optional_descriptor,
                &mut thing,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let mut reference = ptr::dangling_mut();
    // SAFETY: retained thing/token and independent outputs remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_thing_scalar_role_reference(
                thing,
                &optional_role,
                &mut reference,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    assert!(reference.is_null());
    assert!(diagnostics.is_null());
    // SAFETY: this slot uniquely owns the optional-role thing.
    assert_eq!(
        unsafe { type_bridge_projected_thing_close(&mut thing) },
        TypeBridgeStatus::Ok,
    );

    let membership = type_id(TypeKind::Relation, "membership");
    let membership_model = model_token(&fixture.projection, TypeKind::Relation, "membership");
    let member_role = role_token(&fixture.projection, &membership, "member");
    let missing_descriptor = TypeBridgeProjectedThingDescriptorV1 {
        model: &membership_model,
        iid: text("0x41"),
        ..optional_descriptor
    };
    thing = ptr::dangling_mut();
    diagnostics = ptr::null_mut();
    // SAFETY: descriptor inputs and independent output slots remain live.
    let status = unsafe {
        type_bridge_projected_thing_open_v1(
            fixture.package,
            &missing_descriptor,
            &mut thing,
            &mut diagnostics,
        )
    };
    assert!(thing.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::ExecutionFailed,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Integrity,
        "missing_required_role",
    );

    let person_model = model_token(&fixture.projection, TypeKind::Entity, "person");
    let player_descriptor = TypeBridgeProjectedReferenceDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedReferenceDescriptorV1>()).unwrap(),
        version: 1,
        model: &person_model,
        iid: text("0x10"),
        keys: ptr::null(),
        key_count: 0,
        reserved: [0; 4],
    };
    let mut player = ptr::null_mut();
    diagnostics = ptr::null_mut();
    // SAFETY: descriptor inputs and independent output slots remain live.
    assert_eq!(
        unsafe {
            type_bridge_projected_reference_open_v1(
                fixture.package,
                &player_descriptor,
                &mut player,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    let players = [
        player as *const TypeBridgeProjectedReference,
        player as *const TypeBridgeProjectedReference,
    ];
    let roles = [role_input(&member_role, &players)];
    let excess_descriptor = TypeBridgeProjectedThingDescriptorV1 {
        roles: roles.as_ptr(),
        role_count: roles.len(),
        ..missing_descriptor
    };
    thing = ptr::dangling_mut();
    // SAFETY: descriptor graph and independent output slots remain live.
    let status = unsafe {
        type_bridge_projected_thing_open_v1(
            fixture.package,
            &excess_descriptor,
            &mut thing,
            &mut diagnostics,
        )
    };
    assert!(thing.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::ExecutionFailed,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Integrity,
        "role_cardinality_violation",
    );
    // SAFETY: this slot uniquely owns the player reference.
    assert_eq!(
        unsafe { type_bridge_projected_reference_close(&mut player) },
        TypeBridgeStatus::Ok,
    );
}

fn assert_execution_error(
    status: TypeBridgeStatus,
    expected_status: TypeBridgeStatus,
    mut diagnostics: *mut TypeBridgeExecutionDiagnostics,
    expected_category: TypeBridgeExecutionDiagnosticCategory,
    expected_code: &str,
) {
    assert_eq!(status, expected_status);
    assert!(!diagnostics.is_null());
    let mut count = 0;
    // SAFETY: diagnostics is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_count(diagnostics, &mut count) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(count, 1);
    let mut item = TypeBridgeExecutionDiagnosticViewV1 {
        struct_size: 0,
        version: 0,
        category: TypeBridgeExecutionDiagnosticCategory::Internal,
        reserved0: 0,
        code: TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        },
        message: TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        },
        path_count: 0,
        detail_count: 0,
        reserved: [0; 4],
    };
    // SAFETY: diagnostics is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &mut item) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(item.category, expected_category);
    assert_eq!(copied(item.code), expected_code.as_bytes());
    // SAFETY: test owns this exact diagnostics slot.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_close(&mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());
}

#[test]
fn invalid_scalar_token_layout_and_cross_package_inputs_fail_with_typed_diagnostics() {
    let fixture = open_fixture(SOURCE, "abi", "projected-model-invalid");
    let bool_token = model_token(&fixture.projection, TypeKind::Attribute, "enabled");
    let double_token = model_token(&fixture.projection, TypeKind::Attribute, "measure-double");
    let string_token = model_token(&fixture.projection, TypeKind::Attribute, "identifier");
    let mut value = NonNull::<TypeBridgeProjectedValue>::dangling().as_ptr();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    let status = unsafe {
        type_bridge_projected_value_boolean_open(
            fixture.package,
            &bool_token,
            2,
            &mut value,
            &mut diagnostics,
        )
    };
    assert!(value.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_value_boolean_invalid",
    );

    diagnostics = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    let status = unsafe {
        type_bridge_projected_value_double_open(
            fixture.package,
            &double_token,
            f64::INFINITY.to_bits(),
            &mut value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_value_double_non_finite",
    );

    let malformed = [0xff_u8];
    diagnostics = ptr::null_mut();
    // SAFETY: the malformed byte is readable and outputs are writable.
    let status = unsafe {
        type_bridge_projected_value_string_open(
            fixture.package,
            &string_token,
            bytes(&malformed),
            &mut value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_value_text_utf8_invalid",
    );

    let oversized = TypeBridgeByteView {
        data: NonNull::<u8>::dangling().as_ptr(),
        length: MAX_CANONICAL_STRING_BYTES + 1,
    };
    diagnostics = ptr::null_mut();
    // SAFETY: the constructor rejects the byte count before reading the sentinel pointer.
    let status = unsafe {
        type_bridge_projected_value_string_open(
            fixture.package,
            &string_token,
            oversized,
            &mut value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::ResourceLimit,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::ResourceLimit,
        "c_projected_value_text_limit_exceeded",
    );

    let decimal_token = model_token(&fixture.projection, TypeKind::Attribute, "balance");
    diagnostics = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    let status = unsafe {
        type_bridge_projected_value_decimal_open(
            fixture.package,
            &decimal_token,
            text("1.00"),
            &mut value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_value_decimal_not_canonical",
    );

    // A domain mismatch comes from the shared projected-model contract and retains
    // its typed path plus lexically ordered typed details at the C boundary.
    let long_token = model_token(&fixture.projection, TypeKind::Attribute, "measure-long");
    for input in [-11, 11] {
        diagnostics = ptr::null_mut();
        // SAFETY: inputs and outputs satisfy the constructor contract.
        let status = unsafe {
            type_bridge_projected_value_long_open(
                fixture.package,
                &long_token,
                input,
                &mut value,
                &mut diagnostics,
            )
        };
        assert!(value.is_null());
        assert_execution_error(
            status,
            TypeBridgeStatus::InvalidArgument,
            diagnostics,
            TypeBridgeExecutionDiagnosticCategory::InvalidInput,
            "range_constraint_violation",
        );
    }

    diagnostics = ptr::null_mut();
    // SAFETY: inputs and outputs satisfy the constructor contract.
    let status = unsafe {
        type_bridge_projected_value_string_open(
            fixture.package,
            &long_token,
            text("not-a-long"),
            &mut value,
            &mut diagnostics,
        )
    };
    assert_eq!(status, TypeBridgeStatus::InvalidArgument);
    assert!(value.is_null());
    assert!(!diagnostics.is_null());
    let mut summary = TypeBridgeExecutionDiagnosticViewV1 {
        struct_size: 0,
        version: 0,
        category: TypeBridgeExecutionDiagnosticCategory::Internal,
        reserved0: 0,
        code: bytes(&[]),
        message: bytes(&[]),
        path_count: 0,
        detail_count: 0,
        reserved: [0; 4],
    };
    // SAFETY: diagnostics is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &mut summary) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(summary.code), b"wrong_scalar_domain");
    assert_eq!(summary.path_count, 1);
    assert_eq!(summary.detail_count, 2);
    let mut path = TypeBridgeExecutionDiagnosticPathViewV1 {
        struct_size: 0,
        kind: TypeBridgeExecutionDiagnosticPathKind::Unknown,
        index: 0,
        primary: bytes(&[]),
        secondary: bytes(&[]),
        tertiary: bytes(&[]),
        reserved: [0; 4],
    };
    // SAFETY: diagnostics is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_path_get_v1(diagnostics, 0, 0, &mut path) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(path.kind, TypeBridgeExecutionDiagnosticPathKind::Type);
    assert_eq!(copied(path.primary), b"attribute");
    assert_eq!(copied(path.secondary), b"measure-long");
    let mut detail = TypeBridgeExecutionDiagnosticDetailViewV1 {
        struct_size: 0,
        kind: TypeBridgeExecutionDiagnosticDetailKind::Unknown,
        boolean_value: 0,
        reserved0: [0; 7],
        unsigned_value: 0,
        key: bytes(&[]),
        primary: bytes(&[]),
        secondary: bytes(&[]),
        tertiary: bytes(&[]),
        quaternary: bytes(&[]),
        reserved: [0; 4],
    };
    // SAFETY: diagnostics is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_detail_get_v1(diagnostics, 0, 0, &mut detail) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(
        detail.kind,
        TypeBridgeExecutionDiagnosticDetailKind::ValueType
    );
    assert_eq!(copied(detail.key), b"actual_value_type");
    assert_eq!(copied(detail.primary), b"string");
    // SAFETY: diagnostics is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_detail_get_v1(diagnostics, 0, 1, &mut detail) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(detail.key), b"expected_value_type");
    assert_eq!(copied(detail.primary), b"long");
    // SAFETY: test owns this exact diagnostics slot.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_close(&mut diagnostics) },
        TypeBridgeStatus::Ok,
    );

    let mut caller_text = b"ada".to_vec();
    // SAFETY: caller bytes are readable for the call and outputs are writable.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_string_open(
                fixture.package,
                &string_token,
                bytes(&caller_text),
                &mut value,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::Ok,
    );
    caller_text.copy_from_slice(b"zed");
    let mut retained_text = bytes(&[]);
    // SAFETY: the scalar is live and output is writable.
    assert_eq!(
        unsafe { type_bridge_projected_value_text(value, &mut retained_text) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(copied(retained_text), b"ada");
    close_value(&mut value);

    let mut forged_token = string_token;
    forged_token.reserved[0] = 1;
    diagnostics = ptr::null_mut();
    // SAFETY: token/input and outputs are readable/writable for the call.
    let status = unsafe {
        type_bridge_projected_value_string_open(
            fixture.package,
            &forged_token,
            text("ada"),
            &mut value,
            &mut diagnostics,
        )
    };
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_token_layout_invalid",
    );

    let person_model = model_token(&fixture.projection, TypeKind::Entity, "person");
    let malformed_descriptor = TypeBridgeProjectedCreateDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedCreateDescriptorV1>()).unwrap(),
        version: 1,
        model: &person_model,
        fields: ptr::null(),
        field_count: 0,
        roles: ptr::null(),
        role_count: 0,
        reserved: [1, 0, 0, 0],
    };
    let mut malformed_create = NonNull::<TypeBridgeProjectedCreate>::dangling().as_ptr();
    diagnostics = ptr::null_mut();
    // SAFETY: descriptor/output storage is readable/writable for the call.
    let status = unsafe {
        type_bridge_projected_create_open_v1(
            fixture.package,
            &malformed_descriptor,
            &mut malformed_create,
            &mut diagnostics,
        )
    };
    assert!(malformed_create.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::InvalidArgument,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_projected_model_descriptor_invalid",
    );

    let foreign_source = SOURCE.replace(
        "elapsed: { value: duration }",
        "elapsed: { value: duration }\n  extra: { value: string }",
    );
    let foreign = open_fixture(&foreign_source, "foreign", "projected-model-foreign");
    let foreign_string = model_token(&foreign.projection, TypeKind::Attribute, "identifier");
    let mut foreign_value = open_text_value(
        &foreign,
        &foreign_string,
        "grace",
        type_bridge_projected_value_string_open,
    );
    let person = type_id(TypeKind::Entity, "person");
    let identifier_field = field_token(&fixture.projection, &person, "identifier");
    let foreign_values = [foreign_value as *const TypeBridgeProjectedValue];
    let fields = [field_input(&identifier_field, &foreign_values)];
    let descriptor = TypeBridgeProjectedCreateDescriptorV1 {
        struct_size: u32::try_from(size_of::<TypeBridgeProjectedCreateDescriptorV1>()).unwrap(),
        version: 1,
        model: &person_model,
        fields: fields.as_ptr(),
        field_count: fields.len(),
        roles: ptr::null(),
        role_count: 0,
        reserved: [0; 4],
    };
    let mut create = NonNull::<TypeBridgeProjectedCreate>::dangling().as_ptr();
    diagnostics = ptr::null_mut();
    // SAFETY: descriptor graph and outputs remain live for the call.
    let status = unsafe {
        type_bridge_projected_create_open_v1(
            fixture.package,
            &descriptor,
            &mut create,
            &mut diagnostics,
        )
    };
    assert!(create.is_null());
    assert_execution_error(
        status,
        TypeBridgeStatus::ExecutionFailed,
        diagnostics,
        TypeBridgeExecutionDiagnosticCategory::Integrity,
        "generated_token_package_mismatch",
    );
    close_value(&mut foreign_value);

    // Distinct output slots are mandatory and initialized independently.
    let mut shared = NonNull::<TypeBridgeProjectedValue>::dangling().as_ptr();
    let value_slot = &mut shared as *mut *mut TypeBridgeProjectedValue;
    let diagnostics_slot = value_slot.cast::<*mut TypeBridgeExecutionDiagnostics>();
    // SAFETY: deliberately aliased writable slots exercise pre-dispatch rejection.
    assert_eq!(
        unsafe {
            type_bridge_projected_value_string_open(
                fixture.package,
                &string_token,
                text("ada"),
                value_slot,
                diagnostics_slot,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(
        shared,
        NonNull::<TypeBridgeProjectedValue>::dangling().as_ptr()
    );

    // Exact pointer-to-pointer close contracts reject null slots and accept null values.
    // SAFETY: explicit null slot probe.
    assert_eq!(
        unsafe { type_bridge_projected_value_close(ptr::null_mut()) },
        TypeBridgeStatus::InvalidArgument,
    );
    let mut null_value = ptr::null_mut();
    // SAFETY: writable slot contains null.
    assert_eq!(
        unsafe { type_bridge_projected_value_close(&mut null_value) },
        TypeBridgeStatus::Ok,
    );
}
