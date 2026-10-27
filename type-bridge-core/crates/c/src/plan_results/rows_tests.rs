use super::*;
use crate::abi::{
    ABI_MAJOR, ABI_MINOR, TypeBridgeByteView, TypeBridgeSchemaPackage,
    TypeBridgeSchemaPackageDescriptorV1,
};
use crate::entity_crud::tests::{COMMIT_SUCCESS, Response, fixture, model_token};
use crate::plan_builder::bulk::purchase_package;
use crate::plan_builder::{columns::*, context::*, idioms::*, terminals::TypeBridgeHeldPlan, *};
use crate::staged::*;
use type_bridge_contract::id::{AttributeId, TypeKind};
use type_bridge_contract::query_plan::v3::{PlanBuilder, QueryPlanV3};
use type_bridge_orm::session::backend::QueryResult;

fn bytes(value: &[u8]) -> TypeBridgeByteView {
    TypeBridgeByteView {
        data: value.as_ptr(),
        length: value.len(),
    }
}
fn package(fixture: &purchase_package::PurchasePackage) -> TypeBridgeSchemaPackage {
    let authority = type_bridge_schema::encode_schema_authority(&fixture.authority);
    let declared =
        type_bridge_contract::schema::encode_declared_schema(fixture.authority.declared_schema())
            .unwrap();
    let projection = type_bridge_contract::codec::to_canonical_json(&fixture.projection).unwrap();
    let semantic =
        type_bridge_contract::codec::to_canonical_json(fixture.projection.semantic_fingerprint())
            .unwrap();
    let binding =
        type_bridge_contract::codec::to_canonical_json(fixture.projection.projection_fingerprint())
            .unwrap();
    let profile = fixture
        .projection
        .semantic_fingerprint()
        .as_fingerprint()
        .semantic_profile()
        .unwrap();
    crate::schema_package::open(TypeBridgeSchemaPackageDescriptorV1 {
        struct_size: size_of::<TypeBridgeSchemaPackageDescriptorV1>() as u32,
        abi_major: ABI_MAJOR,
        abi_minor: ABI_MINOR,
        schema_authority_json: bytes(&authority),
        declared_schema_json: bytes(&declared),
        runtime_projection_json: bytes(&projection),
        semantic_fingerprint_json: bytes(&semantic),
        binding_fingerprint_json: bytes(&binding),
        managed_scope: bytes(purchase_package::SCOPE.as_bytes()),
        semantic_profile: bytes(profile.as_str().as_bytes()),
        reserved: [0; 4],
    })
    .unwrap()
}
fn response(plan: &QueryPlanV3, occurrences: usize) -> Response {
    if plan.fetch_document_shape().is_some() {
        return Response::Result(QueryResult::Documents(if occurrences == 0 {
            vec![]
        } else {
            vec![
                serde_json::json!({"_iid":"0x01", "_type":"item", "attributes":{"item-id":"I1", "stock":2, "price":3}}),
            ]
        }));
    }
    let row = plan
        .bindings()
        .iter()
        .map(|binding| {
            use type_bridge_contract::query_remote::RemoteValue;
            let value = if let Some(domain) = plan.scalar_value_type(binding.id()) {
                let value = match domain {
                    ValueTypeTag::Long => CanonicalValue::Long(2),
                    ValueTypeTag::String => CanonicalValue::String(
                        type_bridge_contract::value::CanonicalString::new(
                            if binding.variable().as_str().contains("iid") {
                                "0x01"
                            } else {
                                "I1"
                            },
                        )
                        .unwrap(),
                    ),
                    _ => panic!("tuple fixture domain"),
                };
                RemoteValue::Value { value }
            } else {
                RemoteValue::Thing {
                    iid: "0x01".to_owned(),
                    type_id: TypeId::new(TypeKind::Entity, "item").unwrap(),
                }
            };
            (
                binding.variable().as_str().to_owned(),
                serde_json::to_value(value).unwrap(),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Response::Result(QueryResult::Rows(vec![
        serde_json::Value::Object(row);
        occurrences
    ]))
}
fn error(
    status: TypeBridgeStatus,
    diagnostics: &mut *mut TypeBridgeExecutionDiagnostics,
    code: &str,
) {
    assert_ne!(status, TypeBridgeStatus::Ok);
    assert_eq!(crate::entity_crud::tests::diagnostic(*diagnostics).1, code);
    // SAFETY: this helper owns the diagnostic returned by the failed read.
    unsafe { crate::execution_diagnostic::type_bridge_execution_diagnostics_close(diagnostics) };
}

#[test]
fn model_scalar_rows_share_core_bytes_ordinals_and_owner_with_checked_getters() {
    for empty in [false, true] {
        let generated = purchase_package::purchase_package();
        let package = package(&generated);
        let item = TypeId::new(TypeKind::Entity, "item").unwrap();
        let token = model_token(&package, item.clone());
        let digest = type_bridge_contract::projection::c_view::CViewLayouts::for_projection(
            &package.state()._projection,
        )
        .unwrap()
        .digest();
        let mut builder = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: all inputs and writable owner slots are distinct and remain live.
        unsafe {
            assert_eq!(
                type_bridge_plan_builder_new_with_layout(
                    &package,
                    ptr::null(),
                    bytes(&digest),
                    &mut builder,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            let mut model = 0;
            assert_eq!(
                type_bridge_plan_builder_type_token(
                    builder,
                    &token,
                    bytes(b"shop_plan_open"),
                    &mut model
                ),
                TypeBridgeStatus::Ok
            );
            let mut binding = 0;
            assert_eq!(
                type_bridge_plan_builder_exact(builder, model, &mut binding),
                TypeBridgeStatus::Ok
            );
            let stock_index = generated.projection.models()[&item]
                .query_tokens()
                .fields()
                .keys()
                .position(|field| field.attribute().label().as_str() == "stock")
                .unwrap();
            let field =
                type_bridge_plan_builder_model_field(builder, token.ordinal, stock_index as u32);
            let stock = type_bridge_plan_long_field(builder, binding, field);
            let columns = [
                type_bridge_plan_model_column(builder, binding, token.ordinal),
                type_bridge_plan_long_column(stock),
            ];
            let mut mixed = std::mem::MaybeUninit::<TypeBridgeHeldPlan>::uninit();
            assert_eq!(
                type_bridge_plan_returning(
                    builder,
                    columns.as_ptr(),
                    columns.len(),
                    mixed.as_mut_ptr()
                ),
                TypeBridgeStatus::Ok
            );
            let mixed = mixed.assume_init();
            let scalar_column = type_bridge_plan_long_column(stock);
            let mut scalar = std::mem::MaybeUninit::<TypeBridgeHeldPlan>::uninit();
            assert_eq!(
                type_bridge_plan_returning(builder, &scalar_column, 1, scalar.as_mut_ptr()),
                TypeBridgeStatus::Ok
            );
            let scalar = scalar.assume_init();
            let mixed_source = (&mut *builder).finish_selection(mixed.selection).unwrap();
            let scalar_source = (&mut *builder).finish_selection(scalar.selection).unwrap();
            let mut core = PlanBuilder::new();
            let thing = core.exact(item.clone());
            let value = core.field_value(thing, AttributeId::new("stock").unwrap());
            core.returning(vec![thing, value]);
            let core = core
                .build(
                    generated.authority.declared_schema(),
                    generated
                        .authority
                        .managed_state()
                        .managed_semantic_schema()
                        .clone(),
                )
                .unwrap();
            let core_stack = type_bridge_orm::match_request::build_plan_fetch_stack(
                mixed_source.registry.as_ref().unwrap(),
                &core,
                &[(
                    thing,
                    type_bridge_orm::match_request::DescriptorId::new("entity:item"),
                )],
            )
            .unwrap();
            assert_eq!(
                AsRef::<[QueryPlanV3]>::as_ref(mixed_source.as_ref())
                    .iter()
                    .map(|plan| String::from_utf8(plan.canonical_bytes().unwrap()).unwrap())
                    .collect::<Vec<_>>(),
                core_stack
                    .plans()
                    .iter()
                    .map(|plan| String::from_utf8(plan.canonical_bytes().unwrap()).unwrap())
                    .collect::<Vec<_>>(),
            );
            let responses = AsRef::<[QueryPlanV3]>::as_ref(mixed_source.as_ref())
                .iter()
                .enumerate()
                .map(|(index, plan)| {
                    response(
                        plan,
                        if empty {
                            0
                        } else if index == 0 {
                            2
                        } else {
                            1
                        },
                    )
                })
                .chain(
                    AsRef::<[QueryPlanV3]>::as_ref(scalar_source.as_ref())
                        .iter()
                        .map(|plan| response(plan, if empty { 0 } else { 2 })),
                )
                .collect();
            let fixture = fixture(responses, COMMIT_SUCCESS);
            let database = crate::runtime::TypeBridgeDatabase::from_test_database(
                Arc::clone(package.state()),
                fixture.database.orm_database().clone(),
            );
            let mut transaction = ptr::null_mut();
            assert_eq!(
                type_bridge_database_transaction(
                    &database,
                    TYPE_BRIDGE_TRANSACTION_READ,
                    &mut transaction,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            let (mut mixed_pending, mut scalar_pending) = (ptr::null_mut(), ptr::null_mut());
            assert_eq!(
                type_bridge_staged_transaction_stage_selection(
                    transaction,
                    builder,
                    mixed.selection,
                    &mut mixed_pending,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_staged_transaction_stage_selection(
                    transaction,
                    builder,
                    scalar.selection,
                    &mut scalar_pending,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            let mut before = ptr::dangling_mut();
            assert_eq!(
                type_bridge_rows_pending_result(mixed_pending, &mut before, &mut diagnostics),
                TypeBridgeStatus::InUse
            );
            assert!(before.is_null());
            crate::execution_diagnostic::type_bridge_execution_diagnostics_close(&mut diagnostics);
            assert_eq!(fixture.state.query_count(), 0);
            type_bridge_plan_builder_free(&mut builder);
            assert_eq!(
                type_bridge_staged_transaction_commit(transaction, &mut diagnostics),
                TypeBridgeStatus::Ok,
                "{}",
                if diagnostics.is_null() {
                    String::new()
                } else {
                    crate::entity_crud::tests::diagnostic(diagnostics).1
                }
            );
            type_bridge_staged_transaction_close(&mut transaction);
            let (mut rows, mut scalar_rows, mut second) =
                (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            assert_eq!(
                type_bridge_rows_pending_result(mixed_pending, &mut rows, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_rows_pending_result(mixed_pending, &mut second, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_rows_pending_result(scalar_pending, &mut scalar_rows, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            all_scalar_getters(Arc::clone(&(&*rows).state.source));
            type_bridge_pending_free(&mut mixed_pending);
            type_bridge_pending_free(&mut scalar_pending);
            let mut count = usize::MAX;
            assert_eq!(
                type_bridge_rows_result_count(rows, &mut count, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count, if empty { 0 } else { 2 });
            assert_eq!(
                type_bridge_rows_result_column_count(rows, &mut count, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_eq!(count, 2);
            let mut output = 99;
            if empty {
                error(
                    type_bridge_rows_result_long(rows, 0, 1, &mut output, &mut diagnostics),
                    &mut diagnostics,
                    "plan_row_out_of_range",
                );
                assert_eq!(output, 0);
            } else {
                assert_eq!(
                    type_bridge_rows_result_long(rows, 0, 1, &mut output, &mut diagnostics),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(output, 2);
                assert_eq!(
                    type_bridge_rows_result_long(scalar_rows, 1, 0, &mut output, &mut diagnostics),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(output, 2);
                error(
                    type_bridge_rows_result_long(rows, 0, 0, &mut output, &mut diagnostics),
                    &mut diagnostics,
                    "plan_cell_kind_mismatch",
                );
                assert_eq!(output, 0);
                let mut text = ptr::dangling();
                error(
                    type_bridge_rows_result_string(rows, 0, 1, &mut text, &mut diagnostics),
                    &mut diagnostics,
                    "plan_cell_kind_mismatch",
                );
                assert!(text.is_null());
                error(
                    type_bridge_rows_result_long(rows, 2, 1, &mut output, &mut diagnostics),
                    &mut diagnostics,
                    "plan_row_out_of_range",
                );
                error(
                    type_bridge_rows_result_long(rows, 0, 2, &mut output, &mut diagnostics),
                    &mut diagnostics,
                    "plan_column_out_of_range",
                );
                let (mut first_view, mut other_view, mut duplicate_view) =
                    (ptr::null(), ptr::null(), ptr::null());
                assert_eq!(
                    type_bridge_rows_result_model_view(
                        rows,
                        0,
                        0,
                        &token,
                        &mut first_view,
                        &mut diagnostics
                    ),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(
                    type_bridge_rows_result_model_view(
                        rows,
                        1,
                        0,
                        &token,
                        &mut duplicate_view,
                        &mut diagnostics
                    ),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(
                    first_view, duplicate_view,
                    "duplicate selections reuse the core-issued model ordinal"
                );
                assert_eq!(
                    type_bridge_rows_result_model_view(
                        second,
                        0,
                        0,
                        &token,
                        &mut other_view,
                        &mut diagnostics
                    ),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(
                    first_view, other_view,
                    "snapshots reuse the same cached view block"
                );
                error(
                    type_bridge_rows_result_model_view(
                        rows,
                        0,
                        1,
                        &token,
                        &mut first_view,
                        &mut diagnostics,
                    ),
                    &mut diagnostics,
                    "plan_cell_kind_mismatch",
                );
                assert!(first_view.is_null());
                type_bridge_rows_result_free(&mut rows);
                let header = generated.package.get("include/shop/models.h").unwrap();
                let source = "#include <shop/models.h>\n#ifdef __cplusplus\nextern \"C\"\n#endif\nint inspect_view(const void *bytes, int mode) { const shop_item_plan_view_t *item = (const shop_item_plan_view_t *)bytes; (void)mode; return item->has_stock && item->stock == 2 && item->has_itemzhid && strcmp(item->itemzhid, \"I1\") == 0 ? 0 : 1; }\n";
                for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")]
                {
                    let library = super::super::view_tests::Library::compile(
                        compiler, language, standard, "shop", header, source,
                    );
                    library.inspect(other_view.cast(), 0);
                }
            }
            type_bridge_rows_result_free(&mut rows);
            type_bridge_rows_result_free(&mut second);
            type_bridge_rows_result_free(&mut scalar_rows);
        }
    }
}

fn all_scalar_getters(source: Arc<CompiledSelection>) {
    use std::ffi::CStr;
    use std::str::FromStr;
    use type_bridge_contract::temporal::{CanonicalDate, CanonicalDateTime, CanonicalDuration};
    use type_bridge_contract::value::{CanonicalDouble, CanonicalString, DecimalValue};
    let values = [
        CanonicalValue::Long(7),
        CanonicalValue::Double(CanonicalDouble::new(1.25).unwrap()),
        CanonicalValue::Boolean(true),
        CanonicalValue::String(CanonicalString::new("hello").unwrap()),
        CanonicalValue::Date(CanonicalDate::from_str("2024-01-02").unwrap()),
        CanonicalValue::DateTime(CanonicalDateTime::from_str("2024-01-02T03:04:05").unwrap()),
        CanonicalValue::DateTimeTz(
            type_bridge_schema::parse_provider_datetime_tz("2024-01-02T03:04:05Z").unwrap(),
        ),
        CanonicalValue::Decimal(DecimalValue::new("1.25").unwrap()),
        CanonicalValue::Duration(CanonicalDuration::from_str("P1D").unwrap()),
    ];
    let result = TypeBridgeRowsResult {
        state: Arc::new(RowsResultState {
            source,
            models: None,
            cells: vec![
                values
                    .iter()
                    .map(|value| Some(TypedScalar::new(value).unwrap()))
                    .collect(),
            ],
            columns: values.len(),
        }),
    };
    let mut diagnostics = ptr::null_mut();
    let (mut integer, mut double, mut boolean) = (0, 0.0, false);
    // SAFETY: the result and all output slots stay live and disjoint during each read.
    unsafe {
        assert_eq!(
            type_bridge_rows_result_long(&result, 0, 0, &mut integer, &mut diagnostics),
            TypeBridgeStatus::Ok
        );
        assert_eq!(integer, 7);
        assert_eq!(
            type_bridge_rows_result_double(&result, 0, 1, &mut double, &mut diagnostics),
            TypeBridgeStatus::Ok
        );
        assert_eq!(double, 1.25);
        assert_eq!(
            type_bridge_rows_result_bool(&result, 0, 2, &mut boolean, &mut diagnostics),
            TypeBridgeStatus::Ok
        );
        assert!(boolean);
    }
    type Getter = unsafe extern "C" fn(
        *const TypeBridgeRowsResult,
        usize,
        usize,
        *mut *const std::ffi::c_char,
        *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    let getters: [Getter; 6] = [
        type_bridge_rows_result_string,
        type_bridge_rows_result_date,
        type_bridge_rows_result_datetime,
        type_bridge_rows_result_datetime_tz,
        type_bridge_rows_result_decimal,
        type_bridge_rows_result_duration,
    ];
    for (offset, getter) in getters.into_iter().enumerate() {
        let column = offset + 3;
        let mut text = ptr::null();
        // SAFETY: typed getter borrows text from a live owner and initializes disjoint outputs.
        unsafe {
            assert_eq!(
                getter(&result, 0, column, &mut text, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            let Scalar::Text(expected) = &result.state.cells[0][column].as_ref().unwrap().value
            else {
                panic!("text fixture")
            };
            assert_eq!(CStr::from_ptr(text), expected.as_c_str());
            assert!(diagnostics.is_null());
            error(
                getter(&result, 0, 0, &mut text, &mut diagnostics),
                &mut diagnostics,
                "plan_cell_kind_mismatch",
            );
            assert!(text.is_null());
        }
    }
}

#[test]
fn repeated_model_rows_use_proof_ordinals_after_materialization() {
    use crate::plan_builder::selections::type_bridge_plan_builder_hold;
    use type_bridge_contract::projection::c_view::{CViewKey, CViewLayouts, CViewMemberKind};
    let generated = purchase_package::purchase_package();
    let package = package(&generated);
    let item = TypeId::new(TypeKind::Entity, "item").unwrap();
    let token = model_token(&package, item.clone());
    let layouts = Arc::new(CViewLayouts::for_projection(&package.state()._projection).unwrap());
    let exact = layouts
        .get(&CViewKey::Exact {
            model: item.clone(),
            player: false,
        })
        .unwrap();
    let iid_offset = exact
        .members
        .iter()
        .find(|member| matches!(member.kind, CViewMemberKind::Iid))
        .unwrap()
        .offset;
    let stock_offset = exact.members.iter().find(|member| matches!(&member.kind, CViewMemberKind::Field { field, .. } if field.attribute().label().as_str() == "stock")).unwrap().offset;
    let size = exact.size;
    let (mut builder, mut setup_diagnostics) = (ptr::null_mut(), ptr::null_mut());
    let (mixed, single, sources);
    // SAFETY: each live input and writable output is distinct for the public recording calls.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_new_with_layout(
                &package,
                ptr::null(),
                bytes(&layouts.digest()),
                &mut builder,
                &mut setup_diagnostics
            ),
            TypeBridgeStatus::Ok
        );
        let mut model = 0;
        assert_eq!(
            type_bridge_plan_builder_type_token(
                builder,
                &token,
                bytes(b"shop_plan_open"),
                &mut model
            ),
            TypeBridgeStatus::Ok
        );
        let mut binding = 0;
        assert_eq!(
            type_bridge_plan_builder_exact(builder, model, &mut binding),
            TypeBridgeStatus::Ok
        );
        let stock_index = generated.projection.models()[&item]
            .query_tokens()
            .fields()
            .keys()
            .position(|field| field.attribute().label().as_str() == "stock")
            .unwrap();
        let field =
            type_bridge_plan_builder_model_field(builder, token.ordinal, stock_index as u32);
        let columns = [
            type_bridge_plan_model_column(builder, binding, token.ordinal),
            type_bridge_plan_long_column(type_bridge_plan_long_field(builder, binding, field)),
        ];
        let mut held = std::mem::MaybeUninit::<TypeBridgeHeldPlan>::uninit();
        assert_eq!(
            type_bridge_plan_returning(builder, columns.as_ptr(), 2, held.as_mut_ptr()),
            TypeBridgeStatus::Ok
        );
        mixed = held.assume_init().selection;
        let mut held_model = 0;
        assert_eq!(
            type_bridge_plan_builder_hold(builder, 3, binding, token.ordinal, &mut held_model),
            TypeBridgeStatus::Ok
        );
        single = held_model;
        sources = [
            (&mut *builder).finish_selection(mixed).unwrap(),
            (&mut *builder).finish_selection(single).unwrap(),
        ];
    }
    let responses = sources.iter().enumerate().flat_map(|(selection, source)| AsRef::<[QueryPlanV3]>::as_ref(source.as_ref()).iter().map(move |plan| (selection, plan))).map(|(selection, plan)| {
        if plan.fetch_document_shape().is_some() {
            let documents = vec![
                serde_json::json!({"_iid":"0x01", "_type":"item", "attributes":{"item-id":"I1", "stock":10, "price":3}}),
                serde_json::json!({"_iid":"0x02", "_type":"item", "attributes":{"item-id":"I2", "stock":20, "price":3}}),
            ];
            return Response::Result(QueryResult::Documents(if selection == 0 { documents } else { vec![documents[1].clone(), documents[0].clone()] }));
        }
        let Response::Result(QueryResult::Rows(mut rows)) = response(plan, 3) else { unreachable!() };
        for (row, number) in rows.iter_mut().zip([2, 1, 2]) {
            for binding in plan.bindings() {
                let value = &mut row[binding.variable().as_str()];
                if value["kind"] == "thing" { value["iid"] = serde_json::json!(format!("0x0{number}")); }
                else if value["value"]["kind"] == "long" { value["value"]["value"] = serde_json::json!((number * 10).to_string()); }
                else if value["value"]["kind"] == "string" {
                    value["value"]["value"] = serde_json::json!(if binding.variable().as_str().to_ascii_lowercase().contains("iid") { format!("0x0{number}") } else { format!("I{number}") });
                }
            }
        }
        Response::Result(QueryResult::Rows(rows))
    }).collect();
    let fixture = fixture(responses, COMMIT_SUCCESS);
    let database = crate::runtime::TypeBridgeDatabase::from_test_database(
        Arc::clone(package.state()),
        fixture.database.orm_database().clone(),
    );
    let (mut tx, mut diagnostics) = (ptr::null_mut(), ptr::null_mut());
    let mut pending = [ptr::null_mut(); 2];
    // SAFETY: all FFI handles/outputs are distinct live test owners; view offsets come from the shared layout authority.
    unsafe {
        assert_eq!(
            type_bridge_database_transaction(
                &database,
                TYPE_BRIDGE_TRANSACTION_READ,
                &mut tx,
                &mut diagnostics
            ),
            TypeBridgeStatus::Ok
        );
        for (selection, output) in [mixed, single].into_iter().zip(&mut pending) {
            assert_eq!(
                type_bridge_staged_transaction_stage_selection(
                    tx,
                    builder,
                    selection,
                    output,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
        }
        assert_eq!(
            type_bridge_staged_transaction_commit(tx, &mut diagnostics),
            TypeBridgeStatus::Ok,
            "{:?}",
            crate::entity_crud::tests::diagnostic(diagnostics)
        );
        let (mut rows, mut models) = (ptr::null_mut(), ptr::null_mut());
        assert_eq!(
            type_bridge_rows_pending_result(pending[0], &mut rows, &mut diagnostics),
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            super::super::type_bridge_pending_plan_result(
                pending[1],
                &mut models,
                &mut diagnostics
            ),
            TypeBridgeStatus::Ok
        );
        for owner in &mut pending {
            type_bridge_pending_free(owner);
        }
        type_bridge_staged_transaction_close(&mut tx);
        type_bridge_plan_builder_free(&mut builder);
        let mut pointers = Vec::new();
        for (row, number) in [2, 1, 2].into_iter().enumerate() {
            let mut view = ptr::null();
            assert_eq!(
                type_bridge_rows_result_model_view(
                    rows,
                    row,
                    0,
                    &token,
                    &mut view,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            pointers.push(view);
            let mut scalar = 0;
            assert_eq!(
                type_bridge_rows_result_long(rows, row, 1, &mut scalar, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_eq!(scalar, number * 10);
            let mut copied = vec![0u64; size.div_ceil(8)];
            if row < 2 {
                assert_eq!(
                    super::super::type_bridge_plan_result_view(
                        models,
                        row,
                        &token,
                        copied.as_mut_ptr().cast(),
                        size
                    ),
                    TypeBridgeStatus::Ok
                );
            }
            let roots = if row < 2 {
                vec![view.cast::<u8>(), copied.as_ptr().cast::<u8>()]
            } else {
                vec![view.cast::<u8>()]
            };
            for root in roots {
                let iid = root
                    .add(iid_offset)
                    .cast::<*const std::ffi::c_char>()
                    .read_unaligned();
                assert_eq!(
                    std::ffi::CStr::from_ptr(iid).to_str().unwrap(),
                    format!("0x0{number}")
                );
                assert_eq!(
                    root.add(stock_offset).cast::<i64>().read_unaligned(),
                    number * 10
                );
            }
        }
        assert_eq!(pointers[0], pointers[2]);
        assert_ne!(pointers[0], pointers[1]);
        type_bridge_rows_result_free(&mut rows);
        super::super::type_bridge_plan_result_free(&mut models);
    }
}
