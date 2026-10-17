//! The C half of the typed-query parity corpora.
//!
//! Runs, through the public C ABI, every case of
//! `tests/contracts/typed_query/corpus-v1.json`,
//! `tests/contracts/enum-spellings.json`, and
//! `tests/contracts/diagnostic-json.json` that C can express, and asserts
//! the same code and details Python, Node, and Rust report. C reports the SDK
//! category plus the query category as a `query_category` detail; the
//! corpus's `invalid_plan` query category is SDK `invalid_input`. Cases C
//! cannot express are skipped by id with a reason, and the skip lists are
//! exact, so a new corpus case must be placed.

use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;
use std::ptr::{self, NonNull};

use serde_json::{Value, json};
use type_bridge_c::TypeBridgeProjectedTokenV1;
use type_bridge_c::{
    TypeBridgeByteView, TypeBridgeDiagnostics, TypeBridgeExecutionDiagnosticCategory,
    TypeBridgeExecutionDiagnosticDetailKind, TypeBridgeExecutionDiagnosticDetailViewV1,
    TypeBridgeExecutionDiagnosticPathKind, TypeBridgeExecutionDiagnosticPathViewV1,
    TypeBridgeExecutionDiagnosticViewV1, TypeBridgeExecutionDiagnostics, TypeBridgeQuery,
    TypeBridgeQueryBinding, TypeBridgeQueryDescriptorV1, TypeBridgeQueryField,
    TypeBridgeQueryFieldReferenceV1, TypeBridgeQueryOrder, TypeBridgeQueryOrderDescriptorV1,
    TypeBridgeQueryPredicate, TypeBridgeQueryReducerV1, TypeBridgeQuerySelection,
    TypeBridgeQuerySelectionDescriptorV1, TypeBridgeQuerySession, TypeBridgeQueryShapeSlotV1,
    TypeBridgeQueryTerminal, TypeBridgeQueryTerminalDescriptorV1, TypeBridgeSchemaPackage,
    TypeBridgeSchemaPackageDescriptorV1, TypeBridgeStatus,
};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectedTokenIdentity, ProjectedTokenKind, ProjectionConfig,
    RuntimeProjection, TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
};
use type_bridge_contract::schema::{DocumentId, OwnsFactId, encode_declared_schema};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, build_schema_authority,
    encode_schema_authority, normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::CEmitter;

const TYPED_QUERY_CORPUS: &str =
    include_str!("../../../../tests/contracts/typed_query/corpus-v1.json");
const ENUM_SPELLINGS: &str = include_str!("../../../../tests/contracts/enum-spellings.json");
const DIAGNOSTIC_JSON: &str = include_str!("../../../../tests/contracts/diagnostic-json.json");
const HEADER: &str = include_str!("../include/typebridge/type_bridge.h");

const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name: { key: true }
  company:
    owns:
      name: { key: true }
relations:
  employment:
    relates:
      employee: { card: 1 }
      employer: { card: 1 }
plays:
  person:
    employment: [employee]
  company:
    employment: [employer]
"#;

const DESCRIPTOR_VERSION: u32 = 1;
const MATCH_EXACT: u32 = 1;
const SELECTION_ONE: u32 = 1;
const SHAPE_POSITIONAL: u32 = 1;
const TERMINAL_ROWS: u32 = 1;
const TERMINAL_REDUCE: u32 = 5;
const TERMINAL_REDUCE_FIELDS: u32 = 7;
const ROWS_BOUNDED_MANY: u32 = 2;
const REDUCER_COUNT: u32 = 1;
const SORT_ASCENDING: u32 = 1;
const MISSING_REJECT: u32 = 1;

/// The C layout of `type_bridge_query_limit_override_v1_t`.
#[repr(C)]
struct LimitOverride {
    name: *const u8,
    name_length: usize,
    value: u64,
}

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn type_bridge_schema_package_open_v1(
        descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut TypeBridgeDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_close(
        package: *mut *mut TypeBridgeSchemaPackage,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_session_open_with_limits_v1(
        package: *const TypeBridgeSchemaPackage,
        overrides: *const LimitOverride,
        override_count: usize,
        out_session: *mut *mut TypeBridgeQuerySession,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_session_close(
        session: *mut *mut TypeBridgeQuerySession,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_binding_open_v1(
        session: *const TypeBridgeQuerySession,
        model: *const TypeBridgeProjectedTokenV1,
        mode: u32,
        out_binding: *mut *mut TypeBridgeQueryBinding,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_binding_close(
        binding: *mut *mut TypeBridgeQueryBinding,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_field_open(
        binding: *const TypeBridgeQueryBinding,
        field: *const TypeBridgeProjectedTokenV1,
        out_field: *mut *mut TypeBridgeQueryField,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_field_close(field: *mut *mut TypeBridgeQueryField) -> TypeBridgeStatus;
    fn type_bridge_query_order_open_v1(
        descriptor: *const TypeBridgeQueryOrderDescriptorV1,
        out_order: *mut *mut TypeBridgeQueryOrder,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_order_close(order: *mut *mut TypeBridgeQueryOrder) -> TypeBridgeStatus;
    fn type_bridge_query_selection_open_v1(
        descriptor: *const TypeBridgeQuerySelectionDescriptorV1,
        out_selection: *mut *mut TypeBridgeQuerySelection,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_selection_close(
        selection: *mut *mut TypeBridgeQuerySelection,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_open_v1(
        session: *const TypeBridgeQuerySession,
        descriptor: *const TypeBridgeQueryDescriptorV1,
        out_query: *mut *mut TypeBridgeQuery,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_close(query: *mut *mut TypeBridgeQuery) -> TypeBridgeStatus;
    fn type_bridge_query_terminal_open_v1(
        query: *const TypeBridgeQuery,
        descriptor: *const TypeBridgeQueryTerminalDescriptorV1,
        out_terminal: *mut *mut TypeBridgeQueryTerminal,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_terminal_close(
        terminal: *mut *mut TypeBridgeQueryTerminal,
    ) -> TypeBridgeStatus;
    #[allow(clippy::too_many_arguments)]
    fn type_bridge_query_session_reachable(
        session: *const TypeBridgeQuerySession,
        relation: *const TypeBridgeProjectedTokenV1,
        role_from: *const TypeBridgeProjectedTokenV1,
        role_to: *const TypeBridgeProjectedTokenV1,
        source: *const TypeBridgeQueryBinding,
        expected_source_model: *const TypeBridgeProjectedTokenV1,
        expected_source_mode: u32,
        target: *const TypeBridgeQueryBinding,
        expected_target_model: *const TypeBridgeProjectedTokenV1,
        expected_target_mode: u32,
        min_depth: u8,
        max_depth: u8,
        out_predicate: *mut *mut TypeBridgeQueryPredicate,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_query_predicate_close(
        predicate: *mut *mut TypeBridgeQueryPredicate,
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
}

/// One refusal as C reports it: SDK category, code, path, and details in
/// the host JSON spelling.
#[derive(Debug)]
struct Refusal {
    category: TypeBridgeExecutionDiagnosticCategory,
    code: String,
    path: Vec<Value>,
    details: BTreeMap<String, Value>,
}

fn bytes(view: TypeBridgeByteView) -> String {
    if view.length == 0 {
        return String::new();
    }
    // SAFETY: diagnostic views borrow live diagnostic storage.
    let slice = unsafe { std::slice::from_raw_parts(view.data, view.length) };
    String::from_utf8(slice.to_vec()).expect("diagnostic text is UTF-8")
}

/// Read and close the single diagnostic of one failed call.
fn refusal(
    status: TypeBridgeStatus,
    mut diagnostics: *mut TypeBridgeExecutionDiagnostics,
) -> Refusal {
    assert_ne!(status, TypeBridgeStatus::Ok, "the call was not refused");
    assert!(
        !diagnostics.is_null(),
        "refusal {status:?} carries no diagnostics"
    );
    let mut count = 0;
    // SAFETY: the failed call returned a live diagnostics handle.
    unsafe {
        assert_eq!(
            type_bridge_execution_diagnostics_count(diagnostics, &mut count),
            TypeBridgeStatus::Ok
        );
    }
    assert_eq!(count, 1);
    let mut view = TypeBridgeExecutionDiagnosticViewV1 {
        struct_size: size_of::<TypeBridgeExecutionDiagnosticViewV1>() as u32,
        version: 1,
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
    // SAFETY: the handle and output view are live for the call.
    unsafe {
        assert_eq!(
            type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &mut view),
            TypeBridgeStatus::Ok
        );
    }
    let mut path = Vec::new();
    for index in 0..view.path_count {
        let mut segment = TypeBridgeExecutionDiagnosticPathViewV1 {
            struct_size: size_of::<TypeBridgeExecutionDiagnosticPathViewV1>() as u32,
            kind: TypeBridgeExecutionDiagnosticPathKind::Unknown,
            index: 0,
            primary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            secondary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            tertiary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            reserved: [0; 4],
        };
        // SAFETY: the handle and output view are live for the call.
        unsafe {
            assert_eq!(
                type_bridge_execution_diagnostics_path_get_v1(diagnostics, 0, index, &mut segment),
                TypeBridgeStatus::Ok
            );
        }
        path.push(match segment.kind {
            TypeBridgeExecutionDiagnosticPathKind::QueryOperation => json!({"kind": "operation"}),
            TypeBridgeExecutionDiagnosticPathKind::QueryOutput => json!({"kind": "output"}),
            other => json!({"kind": format!("{other:?}"), "index": segment.index}),
        });
    }
    let mut details = BTreeMap::new();
    for index in 0..view.detail_count {
        let mut detail = TypeBridgeExecutionDiagnosticDetailViewV1 {
            struct_size: size_of::<TypeBridgeExecutionDiagnosticDetailViewV1>() as u32,
            kind: TypeBridgeExecutionDiagnosticDetailKind::Unknown,
            boolean_value: 0,
            reserved0: [0; 7],
            unsigned_value: 0,
            key: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            primary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            secondary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            tertiary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            quaternary: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            reserved: [0; 4],
        };
        // SAFETY: the handle and output view are live for the call.
        unsafe {
            assert_eq!(
                type_bridge_execution_diagnostics_detail_get_v1(diagnostics, 0, index, &mut detail),
                TypeBridgeStatus::Ok
            );
        }
        let value = match detail.kind {
            TypeBridgeExecutionDiagnosticDetailKind::Count => {
                json!({"kind": "count", "value": detail.unsigned_value.to_string()})
            }
            TypeBridgeExecutionDiagnosticDetailKind::Text => {
                json!({"kind": "query_identity", "value": bytes(detail.primary)})
            }
            other => json!({"kind": format!("{other:?}")}),
        };
        details.insert(bytes(detail.key), value);
    }
    let result = Refusal {
        category: view.category,
        code: bytes(view.code),
        path,
        details,
    };
    // SAFETY: the handle is closed exactly once through its close family.
    unsafe {
        assert_eq!(
            type_bridge_execution_diagnostics_close(&mut diagnostics),
            TypeBridgeStatus::Ok
        );
    }
    result
}

struct Fixture {
    package: *mut TypeBridgeSchemaPackage,
    projection: RuntimeProjection,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: the package handle is owned by this fixture.
        unsafe { type_bridge_schema_package_close(&mut self.package) };
    }
}

impl Fixture {
    fn new() -> Self {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("c-typed-query-corpus.yaml").unwrap(),
            SCHEMA,
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|id| CapabilityId::new(*id).unwrap())
            .collect();
        let scope = ManagedScopeId::new("c-typed-query-corpus").unwrap();
        let context = ManagedDeltaContext::new(scope.clone(), profile.clone(), available);
        let authority =
            build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap();
        let emitter = CEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new("corpus").unwrap()),
            &emitter.generator_handlers_for(&resolved),
            &emitter.code_resources_for(&resolved).unwrap(),
        )
        .unwrap();
        let authority = encode_schema_authority(&authority);
        let declared = encode_declared_schema(&declared).unwrap();
        let projection_json = to_canonical_json(&projection).unwrap();
        let semantic = to_canonical_json(projection.semantic_fingerprint()).unwrap();
        let binding = to_canonical_json(projection.projection_fingerprint()).unwrap();
        let view = |value: &[u8]| TypeBridgeByteView {
            data: value.as_ptr(),
            length: value.len(),
        };
        let descriptor = TypeBridgeSchemaPackageDescriptorV1 {
            struct_size: size_of::<TypeBridgeSchemaPackageDescriptorV1>() as u32,
            abi_major: 1,
            abi_minor: 6,
            schema_authority_json: view(&authority),
            declared_schema_json: view(&declared),
            runtime_projection_json: view(&projection_json),
            semantic_fingerprint_json: view(&semantic),
            binding_fingerprint_json: view(&binding),
            managed_scope: view(scope.as_str().as_bytes()),
            semantic_profile: view(profile.as_str().as_bytes()),
            reserved: [0; 4],
        };
        let mut package = ptr::null_mut();
        let mut diagnostics = NonNull::<TypeBridgeDiagnostics>::dangling().as_ptr();
        // SAFETY: the descriptor borrows live bytes; both outputs are writable.
        let status = unsafe {
            type_bridge_schema_package_open_v1(&descriptor, &mut package, &mut diagnostics)
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        Self {
            package,
            projection,
        }
    }

    fn token(
        &self,
        kind: ProjectedTokenKind,
        identity: ProjectedTokenIdentity,
    ) -> TypeBridgeProjectedTokenV1 {
        TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
            kind: kind.as_u32(),
            ordinal: self.projection.projected_token_ordinal(&identity).unwrap(),
            projection_digest: self
                .projection
                .projection_fingerprint()
                .as_fingerprint()
                .digest()
                .bytes(),
            reserved: [0; 4],
        }
    }

    fn model(&self, kind: TypeKind, label: &str) -> TypeBridgeProjectedTokenV1 {
        self.token(
            ProjectedTokenKind::Model,
            ProjectedTokenIdentity::Model(TypeId::new(kind, label).unwrap()),
        )
    }

    fn person_name(&self) -> TypeBridgeProjectedTokenV1 {
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let field = OwnsFactId::new(person.clone(), AttributeId::new("name").unwrap()).unwrap();
        self.token(
            ProjectedTokenKind::Field,
            ProjectedTokenIdentity::Field {
                owner: person,
                field,
            },
        )
    }

    fn employment_role(&self, role: &str) -> TypeBridgeProjectedTokenV1 {
        self.token(
            ProjectedTokenKind::Role,
            ProjectedTokenIdentity::Role {
                owner: TypeId::new(TypeKind::Relation, "employment").unwrap(),
                role: RoleId::new("employment", role).unwrap(),
            },
        )
    }

    fn session(&self, selected_slots: Option<u64>) -> Session {
        let name = b"selected_slots";
        let overrides: Vec<_> = selected_slots
            .map(|value| LimitOverride {
                name: name.as_ptr(),
                name_length: name.len(),
                value,
            })
            .into_iter()
            .collect();
        let mut session = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the package and override table are live; outputs are writable.
        let status = unsafe {
            type_bridge_query_session_open_with_limits_v1(
                self.package,
                if overrides.is_empty() {
                    ptr::null()
                } else {
                    overrides.as_ptr()
                },
                overrides.len(),
                &mut session,
                &mut diagnostics,
            )
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        Session {
            session,
            root: ptr::null_mut(),
            owned: Vec::new(),
        }
    }
}

/// One query session plus every handle opened from it, closed on drop.
struct Session {
    session: *mut TypeBridgeQuerySession,
    /// The first selected binding of the last query opened.
    root: *mut TypeBridgeQueryBinding,
    owned: Vec<Box<dyn FnMut()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        for mut close in self.owned.drain(..).rev() {
            close();
        }
        // SAFETY: the session handle is owned here.
        unsafe { type_bridge_query_session_close(&mut self.session) };
    }
}

macro_rules! open {
    ($session:expr, $close:ident, |$out:ident, $diagnostics:ident| $call:expr) => {{
        let mut $out = ptr::null_mut();
        let mut $diagnostics = ptr::null_mut();
        // SAFETY: every borrowed input outlives the call; outputs are writable.
        let status = unsafe { $call };
        assert_eq!(status, TypeBridgeStatus::Ok, "{}", stringify!($close));
        let handle = $out;
        $session.owned.push(Box::new(move || {
            let mut handle = handle;
            // SAFETY: each handle is closed exactly once.
            unsafe { $close(&mut handle) };
        }));
        handle
    }};
}

impl Session {
    fn bind(&mut self, token: &TypeBridgeProjectedTokenV1) -> *mut TypeBridgeQueryBinding {
        let session = self.session;
        open!(self, type_bridge_query_binding_close, |out, diagnostics| {
            type_bridge_query_binding_open_v1(
                session,
                token,
                MATCH_EXACT,
                &mut out,
                &mut diagnostics,
            )
        })
    }

    fn selection(
        &mut self,
        binding: *const TypeBridgeQueryBinding,
        model: &TypeBridgeProjectedTokenV1,
    ) -> *mut TypeBridgeQuerySelection {
        let descriptor = selection_descriptor(binding, model);
        open!(
            self,
            type_bridge_query_selection_close,
            |out, diagnostics| {
                type_bridge_query_selection_open_v1(&descriptor, &mut out, &mut diagnostics)
            }
        )
    }

    /// Open a positional query over `count` fresh `person` slots.
    fn query_status(
        &mut self,
        fixture: &Fixture,
        count: usize,
    ) -> (
        TypeBridgeStatus,
        *mut TypeBridgeQuery,
        *mut TypeBridgeExecutionDiagnostics,
    ) {
        let person = fixture.model(TypeKind::Entity, "person");
        let mut root: *mut TypeBridgeQueryBinding = ptr::null_mut();
        let slots: Vec<_> = (0..count)
            .map(|_| {
                let binding = self.bind(&person);
                if root.is_null() {
                    root = binding;
                }
                let selection = self.selection(binding, &person);
                TypeBridgeQueryShapeSlotV1 {
                    struct_size: size_of::<TypeBridgeQueryShapeSlotV1>() as u32,
                    version: DESCRIPTOR_VERSION,
                    selection,
                    expected_model: &person,
                    expected_mode: MATCH_EXACT,
                    expected_kind: SELECTION_ONE,
                    reserved0: 0,
                    name: TypeBridgeByteView {
                        data: ptr::null(),
                        length: 0,
                    },
                    reserved: [0; 4],
                }
            })
            .collect();
        let descriptor = TypeBridgeQueryDescriptorV1 {
            struct_size: size_of::<TypeBridgeQueryDescriptorV1>() as u32,
            version: DESCRIPTOR_VERSION,
            shape_kind: SHAPE_POSITIONAL,
            reserved0: 0,
            slots: slots.as_ptr(),
            slot_count: slots.len(),
            reserved: [0; 4],
        };
        let mut query = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the descriptor and its slot table are live; outputs are writable.
        let status = unsafe {
            type_bridge_query_open_v1(self.session, &descriptor, &mut query, &mut diagnostics)
        };
        if status == TypeBridgeStatus::Ok {
            let handle = query;
            self.owned.push(Box::new(move || {
                let mut handle = handle;
                // SAFETY: closed exactly once.
                unsafe { type_bridge_query_close(&mut handle) };
            }));
        }
        self.root = root;
        (status, query, diagnostics)
    }
}

fn selection_descriptor(
    binding: *const TypeBridgeQueryBinding,
    model: &TypeBridgeProjectedTokenV1,
) -> TypeBridgeQuerySelectionDescriptorV1 {
    TypeBridgeQuerySelectionDescriptorV1 {
        struct_size: size_of::<TypeBridgeQuerySelectionDescriptorV1>() as u32,
        version: DESCRIPTOR_VERSION,
        binding,
        expected_model: model,
        expected_mode: MATCH_EXACT,
        kind: SELECTION_ONE,
        distinct: 0,
        reserved0: [0; 7],
        orders: ptr::null(),
        order_count: 0,
        reserved: [0; 4],
    }
}

fn terminal(kind: u32) -> TypeBridgeQueryTerminalDescriptorV1 {
    TypeBridgeQueryTerminalDescriptorV1 {
        struct_size: size_of::<TypeBridgeQueryTerminalDescriptorV1>() as u32,
        version: DESCRIPTOR_VERSION,
        kind,
        cardinality: ROWS_BOUNDED_MANY,
        root: ptr::null(),
        expected_root_model: ptr::null(),
        expected_root_mode: 0,
        reserved1: 0,
        orders: ptr::null(),
        order_count: 0,
        offset: 0,
        limit: 0,
        include_total: 0,
        reserved0: [0; 7],
        group_binding: ptr::null(),
        expected_group_model: ptr::null(),
        expected_group_mode: 0,
        reserved2: 0,
        group_fields: ptr::null(),
        group_field_count: 0,
        reducers: ptr::null(),
        reducer_count: 0,
        reserved: [0; 4],
    }
}

fn count_reducer() -> TypeBridgeQueryReducerV1 {
    TypeBridgeQueryReducerV1 {
        struct_size: size_of::<TypeBridgeQueryReducerV1>() as u32,
        version: DESCRIPTOR_VERSION,
        kind: REDUCER_COUNT,
        reserved0: 0,
        input: ptr::null(),
        expected_field: ptr::null(),
        reserved: [0; 4],
    }
}

/// Run one corpus case through the C ABI and return its refusal.
fn run_case(fixture: &Fixture, case_id: &str) -> Refusal {
    let person = fixture.model(TypeKind::Entity, "person");
    let selected_slots = (case_id == "selection.session-slot-ceiling").then_some(4);
    let mut session = fixture.session(selected_slots);
    match case_id {
        "selection.seventeen-slot-rejection" | "selection.session-slot-ceiling" => {
            let count = if selected_slots.is_some() { 5 } else { 17 };
            let (status, _, diagnostics) = session.query_status(fixture, count);
            return refusal(status, diagnostics);
        }
        "reachability.reversed-bounds" | "reachability.depth-ceiling" => {
            let (min_depth, max_depth) = if case_id == "reachability.reversed-bounds" {
                (3, 2)
            } else {
                (0, 65)
            };
            let company = fixture.model(TypeKind::Entity, "company");
            let relation = fixture.model(TypeKind::Relation, "employment");
            let employee = fixture.employment_role("employee");
            let employer = fixture.employment_role("employer");
            let source = session.bind(&person);
            let target = session.bind(&company);
            let mut predicate = ptr::null_mut();
            let mut diagnostics = ptr::null_mut();
            // SAFETY: every token and handle is live; outputs are writable.
            let status = unsafe {
                type_bridge_query_session_reachable(
                    session.session,
                    &relation,
                    &employee,
                    &employer,
                    source,
                    &person,
                    MATCH_EXACT,
                    target,
                    &company,
                    MATCH_EXACT,
                    min_depth,
                    max_depth,
                    &mut predicate,
                    &mut diagnostics,
                )
            };
            if status == TypeBridgeStatus::Ok {
                // SAFETY: closed exactly once.
                unsafe { type_bridge_query_predicate_close(&mut predicate) };
            }
            return refusal(status, diagnostics);
        }
        _ => {}
    }
    let (status, query, _) = session.query_status(fixture, 1);
    assert_eq!(status, TypeBridgeStatus::Ok);
    let root = session.root;
    let name_token = fixture.person_name();
    let name = open!(
        session,
        type_bridge_query_field_close,
        |out, diagnostics| {
            type_bridge_query_field_open(root, &name_token, &mut out, &mut diagnostics)
        }
    );
    let one = [count_reducer()];
    let seventeen = [count_reducer(); 17];
    let group_fields = [TypeBridgeQueryFieldReferenceV1 {
        struct_size: size_of::<TypeBridgeQueryFieldReferenceV1>() as u32,
        version: DESCRIPTOR_VERSION,
        field: name,
        expected_field: &name_token,
        reserved: [0; 4],
    }; 17];
    let mut descriptor = match case_id {
        "bounds.public-invalid-limit" | "bounds.canonical-zero-limit" => terminal(TERMINAL_ROWS),
        "bounds.public-invalid-offset" => TypeBridgeQueryTerminalDescriptorV1 {
            offset: u64::MAX,
            limit: 1,
            ..terminal(TERMINAL_ROWS)
        },
        "bounds.window-overflow" => TypeBridgeQueryTerminalDescriptorV1 {
            offset: u64::MAX - 1,
            limit: 2,
            ..terminal(TERMINAL_ROWS)
        },
        "aggregate.empty-terms" => terminal(TERMINAL_REDUCE),
        "aggregate.seventeen-terms" => TypeBridgeQueryTerminalDescriptorV1 {
            reducers: seventeen.as_ptr(),
            reducer_count: seventeen.len(),
            ..terminal(TERMINAL_REDUCE)
        },
        "aggregate.seventeen-group-keys" => TypeBridgeQueryTerminalDescriptorV1 {
            group_fields: group_fields.as_ptr(),
            group_field_count: group_fields.len(),
            reducers: one.as_ptr(),
            reducer_count: one.len(),
            ..terminal(TERMINAL_REDUCE_FIELDS)
        },
        other => panic!("no C expression for corpus case {other}"),
    };
    if descriptor.kind != TERMINAL_ROWS {
        descriptor.root = root;
        descriptor.expected_root_model = &person;
        descriptor.expected_root_mode = MATCH_EXACT;
    }
    let mut terminal = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: the descriptor and its tables are live; outputs are writable.
    let status = unsafe {
        type_bridge_query_terminal_open_v1(query, &descriptor, &mut terminal, &mut diagnostics)
    };
    if status == TypeBridgeStatus::Ok {
        // SAFETY: closed exactly once.
        unsafe { type_bridge_query_terminal_close(&mut terminal) };
    }
    refusal(status, diagnostics)
}

/// Typed-query corpus cases C does not express, with the reason.
const TYPED_QUERY_SKIPS: &[(&str, &str)] = &[
    (
        "surface.",
        "language facade surface; C has no generated facade",
    ),
    (
        "compat.",
        "language facade surface; C has no generated facade",
    ),
    (
        "binding.",
        "success case; covered by the C query ABI unit tests",
    ),
    (
        "selection.",
        "success or duplicate-handle case; C selections are opaque handles",
    ),
    (
        "topology.",
        "plan shape; covered by the core validator the C ABI calls",
    ),
    (
        "boolean.",
        "plan shape; covered by the core validator the C ABI calls",
    ),
    (
        "references.",
        "plan shape; covered by the core validator the C ABI calls",
    ),
    ("identity.", "execution semantics; needs a provider"),
    (
        "shape.",
        "plan shape; covered by the core validator the C ABI calls",
    ),
    (
        "order.",
        "order semantics; needs ordered fixtures, covered by C unit tests",
    ),
    ("cardinality.", "execution outcome; needs a provider"),
    ("terminal.", "execution outcome; needs a provider"),
    ("strings.", "language string facade"),
    ("transaction.", "language transaction facade"),
    ("preflight.", "execution preflight; needs a provider"),
    ("execution.", "execution outcome; needs a provider"),
];

/// Cases C runs: every window, cap, and depth rule C exposes.
const TYPED_QUERY_CASES: &[&str] = &[
    "selection.seventeen-slot-rejection",
    "selection.session-slot-ceiling",
    "bounds.public-invalid-limit",
    "bounds.public-invalid-offset",
    "bounds.canonical-zero-limit",
    "bounds.window-overflow",
    "aggregate.empty-terms",
    "aggregate.seventeen-terms",
    "aggregate.seventeen-group-keys",
    "reachability.reversed-bounds",
    "reachability.depth-ceiling",
];

fn sdk_category(query_category: &str) -> TypeBridgeExecutionDiagnosticCategory {
    match query_category {
        "invalid_plan" | "invalid_input" => TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        other => panic!("no C category for corpus category {other}"),
    }
}

#[test]
fn window_cap_and_depth_cases_have_the_corpus_codes() {
    let corpus: Value = serde_json::from_str(TYPED_QUERY_CORPUS).unwrap();
    let fixture = Fixture::new();
    let mut unplaced = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        if !TYPED_QUERY_CASES.contains(&id) {
            if !TYPED_QUERY_SKIPS
                .iter()
                .any(|(prefix, _)| id.starts_with(prefix))
            {
                unplaced.push(id.to_owned());
            }
            continue;
        }
        let expected = &case["expected"];
        let refusal = run_case(&fixture, id);
        assert_eq!(
            refusal.code,
            expected["error_code"].as_str().unwrap(),
            "{id}: {refusal:?}"
        );
        assert_eq!(
            refusal.category,
            sdk_category(expected["error_category"].as_str().unwrap()),
            "{id}"
        );
        assert_eq!(
            refusal.details["query_category"]["value"], expected["error_category"],
            "{id}"
        );
    }
    assert!(
        unplaced.is_empty(),
        "place these corpus cases in the C loader: {unplaced:?}"
    );
    for id in TYPED_QUERY_CASES {
        assert!(
            corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|case| case["id"] == *id),
            "corpus no longer has {id}"
        );
    }
}

/// The C half of `group_keys.seventeen` in `diagnostic-json.json`: C
/// reports the same code, path, and details as the host JSON.
#[test]
fn group_key_refusal_matches_the_diagnostic_golden() {
    let golden: Value = serde_json::from_str(DIAGNOSTIC_JSON).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    // The remaining cases are not C-producible: `unknown_enum_value.*` (C
    // enums are integer constants, see below), `canonical_limit.depth` and
    // `integer_range.extremes` (Rust-only mappings), and
    // `contract.malformed_canonical_json` (checked by schema_package_abi.rs).
    let ids: BTreeSet<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from([
            "canonical_limit.depth",
            "contract.malformed_canonical_json",
            "group_keys.seventeen",
            "integer_range.extremes",
            "unknown_enum_value.sort_direction",
        ]),
        "place new diagnostic golden cases in the C loader"
    );
    let expected = &cases
        .iter()
        .find(|case| case["id"] == "group_keys.seventeen")
        .unwrap()["diagnostic"];
    let refusal = run_case(&Fixture::new(), "aggregate.seventeen-group-keys");
    assert_eq!(refusal.code, expected["code"].as_str().unwrap());
    assert_eq!(
        refusal.category,
        sdk_category(expected["sdk_category"].as_str().unwrap())
    );
    assert_eq!(Value::Array(refusal.path), expected["path"]);
    let mut details: serde_json::Map<String, Value> = refusal.details.into_iter().collect();
    // C carries the query category as a detail; the host JSON as a field.
    let query_category = details.remove("query_category").unwrap();
    assert_eq!(query_category["value"], expected["query_category"]);
    assert_eq!(Value::Object(details), expected["details"]);
}

/// `#define TYPE_BRIDGE_QUERY_<PREFIX>_<NAME> ((type)<value>)` in the public
/// header, as lower-case spelling to value.
fn header_constants(prefix: &str) -> BTreeMap<String, u32> {
    let joined = HEADER.replace("\\\n", " ");
    let marker = format!("#define TYPE_BRIDGE_QUERY_{prefix}_");
    joined
        .lines()
        .filter_map(|line| line.trim().strip_prefix(marker.as_str()))
        .map(|rest| {
            let (name, value) = rest.split_once(char::is_whitespace).unwrap();
            let value = value.trim().trim_end_matches(')');
            let value = value.rsplit(')').next().unwrap();
            (name.to_lowercase(), value.parse().unwrap())
        })
        .collect()
}

/// C enum constants the corpus kinds map to, and canonical spellings C does
/// not expose.
const ENUM_CONSTANTS: &[(&str, &str, &[&str])] = &[
    ("comparison_operator", "COMPARE", &[]),
    ("sort_direction", "SORT", &[]),
    ("missing_order", "MISSING", &[]),
    // C has no unordered complete-rows terminal.
    ("row_cardinality", "ROWS", &["complete"]),
    ("reducer", "REDUCER", &[]),
    ("match_mode", "MATCH", &[]),
];

#[test]
fn c_enum_constants_spell_the_corpus_canonical_spellings() {
    let corpus: Value = serde_json::from_str(ENUM_SPELLINGS).unwrap();
    for (kind, prefix, not_exposed) in ENUM_CONSTANTS {
        let Some(case) = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["kind"] == *kind)
        else {
            assert_eq!(*kind, "match_mode", "corpus lost the {kind} case");
            continue;
        };
        let accepted: BTreeSet<&str> = case["expected"]["details"]["accepted"]["value"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let canonical: BTreeSet<&str> = accepted
            .iter()
            .filter(|spelling| {
                type_bridge_orm::canonical_enum_spelling(kind, spelling) == Ok(**spelling)
            })
            .copied()
            .collect();
        let constants = header_constants(prefix);
        let spelled: BTreeSet<&str> = constants.keys().map(String::as_str).collect();
        let exposed: BTreeSet<&str> = canonical
            .iter()
            .filter(|spelling| !not_exposed.contains(spelling))
            .copied()
            .collect();
        assert_eq!(spelled, exposed, "{kind}");
        let values: BTreeSet<u32> = constants.values().copied().collect();
        assert_eq!(
            values,
            (1..=u32::try_from(constants.len()).unwrap()).collect(),
            "{kind} constants are dense from 1"
        );
    }
}

/// An integer outside a C enum's constants is refused, never coerced.
#[test]
fn out_of_range_enum_constants_are_refused() {
    let fixture = Fixture::new();
    let person = fixture.model(TypeKind::Entity, "person");
    let mut session = fixture.session(None);
    let mut binding = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    let mode = u32::try_from(header_constants("MATCH").len()).unwrap() + 1;
    // SAFETY: the session and token are live; outputs are writable.
    let status = unsafe {
        type_bridge_query_binding_open_v1(
            session.session,
            &person,
            mode,
            &mut binding,
            &mut diagnostics,
        )
    };
    assert!(refusal(status, diagnostics).code.starts_with("c_"));

    let root = session.bind(&person);
    let name_token = fixture.person_name();
    let name = open!(
        session,
        type_bridge_query_field_close,
        |out, diagnostics| {
            type_bridge_query_field_open(root, &name_token, &mut out, &mut diagnostics)
        }
    );
    let order = |direction, missing| TypeBridgeQueryOrderDescriptorV1 {
        struct_size: size_of::<TypeBridgeQueryOrderDescriptorV1>() as u32,
        version: DESCRIPTOR_VERSION,
        field: name,
        expected_field: &name_token,
        direction,
        missing,
        reserved0: 0,
        reserved: [0; 4],
    };
    let sort = u32::try_from(header_constants("SORT").len()).unwrap() + 1;
    let missing = u32::try_from(header_constants("MISSING").len()).unwrap() + 1;
    for descriptor in [order(sort, MISSING_REJECT), order(SORT_ASCENDING, missing)] {
        let mut handle = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the descriptor and its handles are live; outputs are writable.
        let status =
            unsafe { type_bridge_query_order_open_v1(&descriptor, &mut handle, &mut diagnostics) };
        if status == TypeBridgeStatus::Ok {
            // SAFETY: closed exactly once.
            unsafe { type_bridge_query_order_close(&mut handle) };
        }
        assert!(refusal(status, diagnostics).code.starts_with("c_"));
    }

    let (status, query, _) = session.query_status(&fixture, 1);
    assert_eq!(status, TypeBridgeStatus::Ok);
    let cardinality = u32::try_from(header_constants("ROWS").len()).unwrap() + 1;
    let reducer = TypeBridgeQueryReducerV1 {
        kind: u32::try_from(header_constants("REDUCER").len()).unwrap() + 1,
        ..count_reducer()
    };
    let rows = TypeBridgeQueryTerminalDescriptorV1 {
        cardinality,
        limit: 1,
        ..terminal(TERMINAL_ROWS)
    };
    let reduce = TypeBridgeQueryTerminalDescriptorV1 {
        root: session.root,
        expected_root_model: &person,
        expected_root_mode: MATCH_EXACT,
        reducers: &reducer,
        reducer_count: 1,
        ..terminal(TERMINAL_REDUCE)
    };
    for descriptor in [rows, reduce] {
        let mut handle = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the descriptor and its tables are live; outputs are writable.
        let status = unsafe {
            type_bridge_query_terminal_open_v1(query, &descriptor, &mut handle, &mut diagnostics)
        };
        if status == TypeBridgeStatus::Ok {
            // SAFETY: closed exactly once.
            unsafe { type_bridge_query_terminal_close(&mut handle) };
        }
        assert!(refusal(status, diagnostics).code.starts_with("c_"));
    }
}
