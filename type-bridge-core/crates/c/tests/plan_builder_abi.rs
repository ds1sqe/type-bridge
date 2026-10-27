//! The ABI 1.7 plan builder: the C purchase gives the hand-built purchase
//! bytes, invalid arguments surface at build in call order, and the
//! pointer contract holds.

use std::ffi::c_void;
#[cfg(unix)]
use std::fs;
use std::mem::size_of;
#[cfg(not(windows))]
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Command;
use std::ptr;

use type_bridge_c::{
    TypeBridgeByteView, TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticViewV1,
    TypeBridgePlanCondition, TypeBridgePlanLong, TypeBridgePlanString,
    TypeBridgeSchemaPackageDescriptorV1, TypeBridgeStatus,
};

#[path = "support/purchase_package.rs"]
mod purchase_package;
#[cfg(not(windows))]
#[path = "support/temp.rs"]
mod temp;

use purchase_package::{PREFIX, PurchasePackage, purchase_package};
#[cfg(not(windows))]
use temp::TempDirectory;

struct AllocationProbe;
thread_local! {
    static ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
// Count only the probing thread, so unrelated parallel tests cannot affect the result.
unsafe impl std::alloc::GlobalAlloc for AllocationProbe {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        // SAFETY: the unchanged allocation contract is forwarded to System.
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: the unchanged allocation contract is forwarded to System.
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        // SAFETY: the unchanged allocation contract is forwarded to System.
        unsafe { std::alloc::System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationProbe = AllocationProbe;

type Builder = c_void;
type Plan = c_void;
type TypeBridgeSchemaPackage = c_void;
type TypeBridgeExecutionDiagnostics = c_void;

const ENTITY: u32 = 1;
const RELATION: u32 = 2;
const BINDING: u32 = 1;
const INPUT: u32 = 2;
const VALUE: u32 = 3;
const EXPR: u32 = 4;
const EQUAL: u32 = 1;
const GREATER: u32 = 5;
const GREATER_OR_EQUAL: u32 = 6;
const SUB: u32 = 2;
const STRING: i32 = 1;

#[repr(C)]
struct FetchRole {
    role: u32,
    required: u8,
}

unsafe extern "C" {
    fn type_bridge_schema_package_authority_json(
        package: *const TypeBridgeSchemaPackage,
        out: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_model_field(b: *mut Builder, model: u32, field: u32) -> u32;

    fn type_bridge_execution_diagnostics_path_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        diagnostic_index: usize,
        path_index: usize,
        out: *mut type_bridge_c::TypeBridgeExecutionDiagnosticPathViewV1,
    ) -> TypeBridgeStatus;

    fn type_bridge_plan_builder_new_with_package(
        package: *const TypeBridgeSchemaPackage,
        options: *const type_bridge_c::TypeBridgePlanOptionsV1,
        out: *mut *mut Builder,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_type_token(
        builder: *mut Builder,
        token: *const type_bridge_c::TypeBridgeProjectedTokenV1,
        opener: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;

    fn type_bridge_plan_builder_by_iid(
        b: *mut Builder,
        owner: u32,
        iid: TypeBridgeByteView,
    ) -> TypeBridgeStatus;

    fn type_bridge_plan_long_literal(value: i64) -> TypeBridgePlanLong;
    fn type_bridge_plan_string_literal(value: TypeBridgeByteView) -> TypeBridgePlanString;
    fn type_bridge_plan_string_key(
        b: *mut Builder,
        owner: u32,
        field: u32,
        value: TypeBridgePlanString,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_long_field(b: *mut Builder, owner: u32, field: u32) -> TypeBridgePlanLong;
    fn type_bridge_plan_long_ge(
        left: TypeBridgePlanLong,
        right: TypeBridgePlanLong,
    ) -> TypeBridgePlanCondition;
    fn type_bridge_plan_long_gt(
        left: TypeBridgePlanLong,
        right: TypeBridgePlanLong,
    ) -> TypeBridgePlanCondition;
    fn type_bridge_plan_long_add(
        left: TypeBridgePlanLong,
        right: TypeBridgePlanLong,
    ) -> TypeBridgePlanLong;
    fn type_bridge_plan_long_mul(
        left: TypeBridgePlanLong,
        right: TypeBridgePlanLong,
    ) -> TypeBridgePlanLong;
    fn type_bridge_plan_long_sub(
        left: TypeBridgePlanLong,
        right: TypeBridgePlanLong,
    ) -> TypeBridgePlanLong;
    fn type_bridge_plan_long_set(
        b: *mut Builder,
        owner: u32,
        field: u32,
        value: TypeBridgePlanLong,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_require(
        b: *mut Builder,
        condition: TypeBridgePlanCondition,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_count(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        count: *mut usize,
    ) -> TypeBridgeStatus;

    fn type_bridge_plan_builder_fetch(
        b: *mut Builder,
        thing: u32,
        roles: *const FetchRole,
        count: usize,
    ) -> TypeBridgeStatus;

    fn type_bridge_schema_package_open_v1(
        descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut c_void,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_close(
        package: *mut *mut TypeBridgeSchemaPackage,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        index: usize,
        out: *mut TypeBridgeExecutionDiagnosticViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_close(
        diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_v3_canonical_bytes(
        plan: *const Plan,
        buffer: *mut u8,
        capacity: usize,
        out_len: *mut usize,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_free(plan: *mut *mut Plan);

    fn type_bridge_plan_builder_new(out: *mut *mut Builder) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_free(builder: *mut *mut Builder);
    fn type_bridge_plan_builder_type(
        b: *mut Builder,
        kind: u32,
        label: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_owns(
        b: *mut Builder,
        owner: u32,
        attribute: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_role(
        b: *mut Builder,
        relation: u32,
        role: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_string(
        b: *mut Builder,
        input: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_long(
        b: *mut Builder,
        input: i64,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_double(
        b: *mut Builder,
        bits: u64,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_boolean(
        b: *mut Builder,
        input: u8,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_date(
        b: *mut Builder,
        input: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_datetime(
        b: *mut Builder,
        input: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_datetime_tz(
        b: *mut Builder,
        input: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_decimal(
        b: *mut Builder,
        input: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_value_duration(
        b: *mut Builder,
        input: TypeBridgeByteView,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_exact(b: *mut Builder, ty: u32, out: *mut u32) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_field(
        b: *mut Builder,
        owner: u32,
        field: u32,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_compare(
        b: *mut Builder,
        left_kind: u32,
        left: u32,
        comparator: u32,
        right_kind: u32,
        right: u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_input(
        b: *mut Builder,
        name: TypeBridgeByteView,
        value_type: i32,
        optional: u8,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_let(
        b: *mut Builder,
        name: TypeBridgeByteView,
        kind: u32,
        id: u32,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_expr_arith(
        b: *mut Builder,
        op: u32,
        left_kind: u32,
        left: u32,
        right_kind: u32,
        right: u32,
        out: *mut u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_update(
        b: *mut Builder,
        owner: u32,
        field: u32,
        kind: u32,
        operand: u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_insert(b: *mut Builder, ty: u32, out: *mut u32)
    -> TypeBridgeStatus;
    fn type_bridge_plan_builder_thing_field(
        b: *mut Builder,
        thing: u32,
        field: u32,
        kind: u32,
        operand: u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_thing_player(
        b: *mut Builder,
        thing: u32,
        role: u32,
        player: u32,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_row(
        b: *mut Builder,
        values: *const u32,
        count: usize,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_returning(
        b: *mut Builder,
        bindings: *const u32,
        count: usize,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_count(b: *mut Builder) -> TypeBridgeStatus;
    fn type_bridge_plan_builder_build(
        b: *const Builder,
        package: *const TypeBridgeSchemaPackage,
        out_plan: *mut *mut Plan,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
}

fn view(bytes: &[u8]) -> TypeBridgeByteView {
    TypeBridgeByteView {
        data: bytes.as_ptr(),
        length: bytes.len(),
    }
}

/// Bytes of one generated `static const uint8_t` array (or its chunks).
fn emitted_array(source: &str, name: &str) -> Vec<u8> {
    fn parse(source: &str, symbol: &str) -> Option<Vec<u8>> {
        let marker = format!("static const uint8_t {symbol}[] = {{\n");
        let body = source.split_once(&marker)?.1.split_once("};\n")?.0;
        Some(
            body.split([',', '\n'])
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| {
                    let hex = item
                        .strip_prefix("0x")
                        .and_then(|item| item.strip_suffix('u'))
                        .expect("generated byte literal");
                    u8::from_str_radix(hex, 16).expect("generated byte literal is hex")
                })
                .collect(),
        )
    }
    parse(source, name).unwrap_or_else(|| {
        (0..)
            .map_while(|index| parse(source, &format!("{name}_chunk_{index}")))
            .flatten()
            .collect()
    })
}

/// An open package handle over the emitted purchase descriptor bytes.
struct OpenPackage {
    handle: *mut TypeBridgeSchemaPackage,
    _bytes: Vec<Vec<u8>>,
}

impl OpenPackage {
    fn new(fixture: &PurchasePackage) -> Self {
        let source = fixture.source("src/models.c");
        let bytes: Vec<Vec<u8>> = [
            "schema_authority_json",
            "declared_schema_json",
            "runtime_projection_json",
            "semantic_fingerprint_json",
            "binding_fingerprint_json",
            "managed_scope",
            "semantic_profile",
        ]
        .iter()
        .map(|name| emitted_array(source, &format!("{PREFIX}_{name}")))
        .collect();
        let descriptor = TypeBridgeSchemaPackageDescriptorV1 {
            struct_size: u32::try_from(size_of::<TypeBridgeSchemaPackageDescriptorV1>()).unwrap(),
            abi_major: 1,
            abi_minor: 7,
            schema_authority_json: view(&bytes[0]),
            declared_schema_json: view(&bytes[1]),
            runtime_projection_json: view(&bytes[2]),
            semantic_fingerprint_json: view(&bytes[3]),
            binding_fingerprint_json: view(&bytes[4]),
            managed_scope: view(&bytes[5]),
            semantic_profile: view(&bytes[6]),
            reserved: [0; 4],
        };
        let mut handle = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: the descriptor views and both output slots live for the call.
        let status = unsafe {
            type_bridge_schema_package_open_v1(&descriptor, &mut handle, &mut diagnostics)
        };
        assert_eq!(status, TypeBridgeStatus::Ok, "the purchase package opens");
        Self {
            handle,
            _bytes: bytes,
        }
    }
}

impl Drop for OpenPackage {
    fn drop(&mut self) {
        // SAFETY: the package handle is owned here.
        unsafe { type_bridge_schema_package_close(&mut self.handle) };
    }
}

/// Owned builder whose calls must keep the pointer contract.
struct B(*mut Builder);

impl B {
    fn new() -> Self {
        let mut builder = ptr::null_mut();
        // SAFETY: one writable owner slot.
        assert_eq!(
            unsafe { type_bridge_plan_builder_new(&mut builder) },
            TypeBridgeStatus::Ok
        );
        Self(builder)
    }

    /// Run a call that writes one id and expect `OK`.
    fn id(&self, call: impl FnOnce(*mut Builder, *mut u32) -> TypeBridgeStatus) -> u32 {
        let mut out = u32::MAX;
        assert_eq!(call(self.0, &mut out), TypeBridgeStatus::Ok);
        out
    }

    fn ok(&self, call: impl FnOnce(*mut Builder) -> TypeBridgeStatus) {
        assert_eq!(call(self.0), TypeBridgeStatus::Ok);
    }

    fn text(&self, label: &str) -> TypeBridgeByteView {
        view(label.as_bytes())
    }

    /// Build and return the canonical bytes, or the status and first code.
    fn build(&self, package: &OpenPackage) -> Result<Vec<u8>, (TypeBridgeStatus, String)> {
        let mut plan = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: live handles and two distinct output slots.
        let status = unsafe {
            type_bridge_plan_builder_build(self.0, package.handle, &mut plan, &mut diagnostics)
        };
        if status != TypeBridgeStatus::Ok {
            assert!(plan.is_null() && !diagnostics.is_null());
            // SAFETY: the diagnostics handle is live; the view borrows it.
            let code = unsafe {
                let mut item =
                    std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
                assert_eq!(
                    type_bridge_execution_diagnostics_get_v1(diagnostics, 0, item.as_mut_ptr()),
                    TypeBridgeStatus::Ok
                );
                let item = item.assume_init();
                let code = std::slice::from_raw_parts(item.code.data, item.code.length).to_vec();
                type_bridge_execution_diagnostics_close(&mut diagnostics);
                String::from_utf8(code).unwrap()
            };
            return Err((status, code));
        }
        assert!(diagnostics.is_null());
        let mut length = 0;
        // SAFETY: a live plan; a length query writes no bytes.
        unsafe { type_bridge_plan_v3_canonical_bytes(plan, ptr::null_mut(), 0, &mut length) };
        let mut bytes = vec![0; length];
        // SAFETY: the buffer holds `length` writable bytes.
        let status = unsafe {
            type_bridge_plan_v3_canonical_bytes(plan, bytes.as_mut_ptr(), length, &mut length)
        };
        assert_eq!(status, TypeBridgeStatus::Ok);
        // SAFETY: the plan handle is owned here.
        unsafe { type_bridge_plan_free(&mut plan) };
        Ok(bytes)
    }
}

impl Drop for B {
    fn drop(&mut self) {
        // SAFETY: the builder handle is owned here.
        unsafe { type_bridge_plan_builder_free(&mut self.0) };
        assert!(self.0.is_null());
    }
}

/// The purchase through the exported entry points, in the order of
/// `support/purchase_builder.c`.
#[allow(clippy::too_many_lines)]
fn built_purchase(package: &OpenPackage, order: &str, item_id: &str, account_id: &str) -> Vec<u8> {
    let p = B::new();
    // SAFETY (every call below): a live builder, valid views and one
    // writable output slot per call.
    unsafe {
        let ty = |kind, label: &str| {
            p.id(|b, out| type_bridge_plan_builder_type(b, kind, p.text(label), out))
        };
        let item_type = ty(ENTITY, "item");
        let account_type = ty(ENTITY, "account");
        let purchase_type = ty(RELATION, "purchase");
        let owns = |owner, attribute: &str| {
            p.id(|b, out| type_bridge_plan_builder_owns(b, owner, p.text(attribute), out))
        };
        let item_key_field = owns(item_type, "item-id");
        let account_key_field = owns(account_type, "account-id");
        let stock_field = owns(item_type, "stock");
        let price_field = owns(item_type, "price");
        let balance_field = owns(account_type, "balance");
        let order_field = owns(purchase_type, "order-id");
        let role = |label: &str| {
            p.id(|b, out| type_bridge_plan_builder_role(b, purchase_type, p.text(label), out))
        };
        let buyer_role = role("buyer");
        let item_role = role("item");

        let item = p.id(|b, out| type_bridge_plan_builder_exact(b, item_type, out));
        let account = p.id(|b, out| type_bridge_plan_builder_exact(b, account_type, out));
        let input = |name: &str| {
            p.id(|b, out| type_bridge_plan_builder_input(b, p.text(name), STRING, 0, out))
        };
        let order_input = input("order_id");
        let item_input = input("item_id");
        let account_input = input("account_id");
        let field =
            |owner, field| p.id(|b, out| type_bridge_plan_builder_field(b, owner, field, out));
        let item_key = field(item, item_key_field);
        let account_key = field(account, account_key_field);
        let stock = field(item, stock_field);
        let price = field(item, price_field);
        let balance = field(account, balance_field);
        let zero = p.id(|b, out| type_bridge_plan_builder_value_long(b, 0, out));
        let one = p.id(|b, out| type_bridge_plan_builder_value_long(b, 1, out));
        let compare =
            |lk, l, c, rk, r| p.ok(|b| type_bridge_plan_builder_compare(b, lk, l, c, rk, r));
        compare(BINDING, item_key, EQUAL, INPUT, item_input);
        compare(BINDING, account_key, EQUAL, INPUT, account_input);
        compare(BINDING, stock, GREATER, VALUE, zero);
        compare(BINDING, balance, GREATER_OR_EQUAL, BINDING, price);
        let sub = |lk, l, rk, r| {
            p.id(|b, out| type_bridge_plan_builder_expr_arith(b, SUB, lk, l, rk, r, out))
        };
        let let_ = |name: &str, expr| {
            p.id(|b, out| type_bridge_plan_builder_let(b, p.text(name), EXPR, expr, out))
        };
        let next_stock = let_("next_stock", sub(BINDING, stock, VALUE, one));
        let next_balance = let_("next_balance", sub(BINDING, balance, BINDING, price));
        p.ok(|b| type_bridge_plan_builder_update(b, item, stock_field, BINDING, next_stock));
        p.ok(|b| type_bridge_plan_builder_update(b, account, balance_field, BINDING, next_balance));
        let purchase = p.id(|b, out| type_bridge_plan_builder_insert(b, purchase_type, out));
        p.ok(|b| {
            type_bridge_plan_builder_thing_field(b, purchase, order_field, INPUT, order_input)
        });
        p.ok(|b| type_bridge_plan_builder_thing_player(b, purchase, buyer_role, account));
        p.ok(|b| type_bridge_plan_builder_thing_player(b, purchase, item_role, item));
        let text = |value: &str| {
            p.id(|b, out| type_bridge_plan_builder_value_string(b, p.text(value), out))
        };
        let row = [text(order), text(item_id), text(account_id)];
        p.ok(|b| type_bridge_plan_builder_row(b, row.as_ptr(), row.len()));
        p.build(package).expect("the purchase builds")
    }
}

#[test]
fn c_builder_purchase_bytes_equal_the_hand_built_plan() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let built = built_purchase(&package, "O1", "I1", "A1");
    let hand = fixture.hand_built("O1", "I1", "A1");
    assert_eq!(
        String::from_utf8(built).unwrap(),
        String::from_utf8(hand.canonical_bytes().unwrap()).unwrap()
    );
}

#[test]
fn invalid_arguments_surface_at_build_first_in_call_order() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);

    // An unknown kind constant; later invalid calls do not replace it.
    let p = B::new();
    // SAFETY (every call below): a live builder, valid views, one slot.
    unsafe {
        p.id(|b, out| type_bridge_plan_builder_type(b, 9, p.text("item"), out));
        p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("bad label!"), out));
    }
    assert_eq!(
        p.build(&package),
        Err((
            TypeBridgeStatus::InvalidArgument,
            "c_plan_builder_constant_invalid".to_owned()
        ))
    );

    // Unknown ids, a non-canonical value and an expression as a plain operand.
    for (call, code) in [
        (
            Box::new(|p: &B| unsafe {
                p.id(|b, out| type_bridge_plan_builder_exact(b, 7, out));
            }) as Box<dyn Fn(&B)>,
            "c_plan_builder_id_unknown",
        ),
        (
            Box::new(|p: &B| unsafe {
                p.id(|b, out| type_bridge_plan_builder_value_decimal(b, p.text("1.50"), out));
            }),
            "c_plan_builder_value_invalid",
        ),
        (
            Box::new(|p: &B| unsafe {
                p.ok(|b| type_bridge_plan_builder_compare(b, EXPR, 0, EQUAL, VALUE, 0));
            }),
            "c_plan_builder_operand_invalid",
        ),
    ] {
        let p = B::new();
        call(&p);
        assert_eq!(
            p.build(&package),
            Err((TypeBridgeStatus::InvalidArgument, code.to_owned()))
        );
    }

    // The core builder's own rules: fresh names, and no count on a write.
    let p = B::new();
    // SAFETY (every call below): a live builder, valid views, one slot.
    unsafe {
        p.id(|b, out| type_bridge_plan_builder_input(b, p.text("x"), STRING, 0, out));
        p.id(|b, out| type_bridge_plan_builder_input(b, p.text("x"), STRING, 0, out));
    }
    assert_eq!(
        p.build(&package),
        Err((
            TypeBridgeStatus::InvalidArgument,
            "query_plan_builder_duplicate_name".to_owned()
        ))
    );
    let p = B::new();
    // SAFETY (every call below): a live builder, valid views, one slot.
    unsafe {
        let purchase =
            p.id(|b, out| type_bridge_plan_builder_type(b, RELATION, p.text("purchase"), out));
        p.id(|b, out| type_bridge_plan_builder_insert(b, purchase, out));
        p.ok(|b| type_bridge_plan_builder_count(b));
    }
    assert_eq!(
        p.build(&package),
        Err((
            TypeBridgeStatus::InvalidArgument,
            "query_plan_builder_write_operation".to_owned()
        ))
    );
}

#[test]
fn reads_count_and_return_rows_and_typed_values_register() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let p = B::new();
    // SAFETY (every call below): a live builder, valid views, one slot.
    let item = unsafe {
        let item_type =
            p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
        p.id(|b, out| type_bridge_plan_builder_exact(b, item_type, out))
    };
    // SAFETY: as above; the binding array outlives the call.
    unsafe { p.ok(|b| type_bridge_plan_builder_returning(b, [item].as_ptr(), 1)) };
    let rows = p.build(&package).expect("a read plan builds");
    assert!(
        String::from_utf8(rows)
            .unwrap()
            .contains(r#""operation":{"kind":"rows"}"#)
    );
    // SAFETY: as above.
    unsafe { p.ok(|b| type_bridge_plan_builder_count(b)) };
    let count = p.build(&package).expect("a count plan builds");
    assert!(
        String::from_utf8(count)
            .unwrap()
            .contains(r#""operation":{"kind":"count"}"#)
    );

    // Every typed value constructor accepts its canonical form.
    let p = B::new();
    // SAFETY (every call below): a live builder, valid views, one slot.
    let ids = unsafe {
        [
            p.id(|b, out| type_bridge_plan_builder_value_string(b, p.text("text"), out)),
            p.id(|b, out| type_bridge_plan_builder_value_long(b, -7, out)),
            p.id(|b, out| type_bridge_plan_builder_value_double(b, 1.5f64.to_bits(), out)),
            p.id(|b, out| type_bridge_plan_builder_value_boolean(b, 1, out)),
            p.id(|b, out| type_bridge_plan_builder_value_date(b, p.text("2026-08-11"), out)),
            p.id(|b, out| {
                type_bridge_plan_builder_value_datetime(b, p.text("2026-08-11T07:30:00.5"), out)
            }),
            p.id(|b, out| {
                type_bridge_plan_builder_value_datetime_tz(b, p.text("2026-08-11T07:30:00Z"), out)
            }),
            p.id(|b, out| type_bridge_plan_builder_value_decimal(b, p.text("12.5"), out)),
            p.id(|b, out| {
                type_bridge_plan_builder_value_duration(b, p.text("P1M2DT3.000000004S"), out)
            }),
        ]
    };
    assert_eq!(ids, [0, 1, 2, 3, 4, 5, 6, 7, 8], "value ids are dense");
    // SAFETY: as above.
    unsafe {
        let item_type =
            p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
        let item = p.id(|b, out| type_bridge_plan_builder_exact(b, item_type, out));
        p.ok(|b| type_bridge_plan_builder_returning(b, [item].as_ptr(), 1));
    }
    p.build(&package)
        .expect("canonical values of every type leave the builder valid");
    // A Boolean other than zero or one is not canonical.
    // SAFETY: as above.
    unsafe { p.id(|b, out| type_bridge_plan_builder_value_boolean(b, 2, out)) };
    assert_eq!(
        p.build(&package),
        Err((
            TypeBridgeStatus::InvalidArgument,
            "c_plan_builder_value_invalid".to_owned()
        ))
    );
}

#[test]
fn pointer_contract_rejects_before_any_write() {
    let p = B::new();
    let label = b"item";
    // A null builder or output slot.
    let mut out = 9u32;
    // SAFETY: null pointers are rejected by contract.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_type(ptr::null_mut(), ENTITY, view(label), &mut out),
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(out, 0, "the fenced output is cleared first");
        assert_eq!(
            type_bridge_plan_builder_type(p.0, ENTITY, view(label), ptr::null_mut()),
            TypeBridgeStatus::InvalidArgument
        );
        // A view with null data and a non-zero length.
        out = 9;
        let hostile = TypeBridgeByteView {
            data: ptr::null(),
            length: 3,
        };
        assert_eq!(
            type_bridge_plan_builder_type(p.0, ENTITY, hostile, &mut out),
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(out, 0);
        // An output inside the input label is rejected and not written.
        let mut text = *b"itemXXXX";
        let alias = text.as_mut_ptr().cast::<u32>();
        assert_eq!(
            type_bridge_plan_builder_type(p.0, ENTITY, view(&text), alias),
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&text, b"itemXXXX");
        // An unaligned output slot is accepted.
        let mut slots = [0u8; 8];
        let unaligned = slots.as_mut_ptr().add(1).cast::<u32>();
        assert_eq!(
            type_bridge_plan_builder_type(p.0, ENTITY, view(label), unaligned),
            TypeBridgeStatus::Ok
        );
        // A null row array with a non-zero count.
        assert_eq!(
            type_bridge_plan_builder_row(p.0, ptr::null(), 1),
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            type_bridge_plan_builder_row(p.0, ptr::null(), 0),
            TypeBridgeStatus::Ok
        );
        // Free accepts a null owner slot and a null handle.
        type_bridge_plan_builder_free(ptr::null_mut());
        let mut none = ptr::null_mut();
        type_bridge_plan_builder_free(&mut none);
    }
}

/// Strict C17/C++17 compile of the shared C purchase program.
#[cfg(not(windows))]
#[test]
fn c_purchase_builder_compiles_under_strict_c17_and_cpp17() {
    let stage = TempDirectory::new("plan_builder");
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let support = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support");
    let source = support.join("purchase_builder.c");
    for (compiler, standard, language) in [
        ("gcc", "c17", "c"),
        ("clang", "c17", "c"),
        ("g++", "c++17", "c++"),
        ("clang++", "c++17", "c++"),
    ] {
        if cfg!(target_os = "macos") && compiler.starts_with('g') {
            continue;
        }
        let object = stage.path().join(format!("purchase-{compiler}.o"));
        let output = Command::new(compiler)
            .arg(format!("-std={standard}"))
            .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors", "-c"])
            .args(["-x", language])
            .arg("-I")
            .arg(&include)
            .arg("-I")
            .arg(&support)
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap_or_else(|error| panic!("{compiler} is required: {error}"));
        assert!(
            output.status.success(),
            "{compiler} {standard} rejected the purchase builder:\n{}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[cfg(unix)]
fn native_library() -> Option<PathBuf> {
    match std::env::var("TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER").as_deref() {
        Ok("1") => {}
        _ => return None,
    }
    if let Some(path) = std::env::var_os("TYPE_BRIDGE_C_SHARED_LIBRARY") {
        return Some(PathBuf::from(path));
    }
    let filename = format!(
        "{}type_bridge_c{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let found = std::env::current_exe()
        .expect("test executable path")
        .ancestors()
        .skip(1)
        .take(6)
        .map(|directory| directory.join(&filename))
        .find(|candidate| candidate.is_file());
    Some(found.expect("run `cargo build -p type-bridge-c --lib` before the shared consumer tests"))
}

/// The compiled C purchase, linked against the shared library and the
/// generated purchase package, prints the hand-built purchase bytes.
#[cfg(unix)]
#[test]
fn compiled_c_purchase_prints_the_hand_built_bytes() {
    let Some(library) = native_library() else {
        return;
    };
    let fixture = purchase_package();
    let stage = TempDirectory::new("plan_builder_golden");
    for (path, bytes) in fixture.package.files() {
        let target = stage.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let main = stage.path().join("main.c");
    fs::write(
        &main,
        format!(
            r#"#include <stdio.h>
#include <stdlib.h>
#include <typebridge/type_bridge.h>
#include <{PREFIX}/models.h>
#include "purchase_builder.h"

int main(void) {{
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_plan_t *plan = NULL;
  uint8_t *bytes;
  size_t length = 0u, index;
  if ({PREFIX}_schema_package_open(&package, &package_diagnostics) != TYPE_BRIDGE_STATUS_OK) return 2;
  if (purchase_build(package, "O1", "I1", "A1", &plan, &diagnostics) != TYPE_BRIDGE_STATUS_OK) return 3;
  (void)type_bridge_plan_v3_canonical_bytes(plan, NULL, 0u, &length);
  bytes = (uint8_t *)malloc(length);
  if (bytes == NULL || type_bridge_plan_v3_canonical_bytes(plan, bytes, length, &length) != TYPE_BRIDGE_STATUS_OK) return 4;
  for (index = 0u; index < length; ++index) putchar(bytes[index]);
  free(bytes);
  type_bridge_plan_free(&plan);
  return type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK ? 0 : 5;
}}
"#
        ),
    )
    .unwrap();
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let support = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support");
    let directory = library.parent().unwrap();
    let executable = stage.path().join("purchase");
    let compile = Command::new("cc")
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg("-I")
        .arg(&include)
        .arg("-I")
        .arg(&support)
        .arg("-I")
        .arg(stage.path().join("include"))
        .arg(&main)
        .arg(support.join("purchase_builder.c"))
        .arg(stage.path().join("src/models.c"))
        .arg("-L")
        .arg(directory)
        .arg("-ltype_bridge_c")
        .arg(format!("-Wl,-rpath,{}", directory.display()))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("a C compiler is required");
    assert!(
        compile.status.success(),
        "the C purchase failed to build:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("the C purchase runs");
    assert!(
        run.status.success(),
        "the C purchase failed with {}",
        run.status
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        String::from_utf8(
            fixture
                .hand_built("O1", "I1", "A1")
                .canonical_bytes()
                .unwrap()
        )
        .unwrap()
    );
}

#[test]
fn c_fetch_bytes_equal_the_core_builder_and_terminal_errors_surface() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    for required in [false, true] {
        let p = B::new();
        // SAFETY: live handles, readable arrays and distinct writable outputs.
        unsafe {
            let ty =
                p.id(|b, out| type_bridge_plan_builder_type(b, RELATION, p.text("purchase"), out));
            let thing = p.id(|b, out| type_bridge_plan_builder_exact(b, ty, out));
            let field =
                p.id(|b, out| type_bridge_plan_builder_owns(b, ty, p.text("order-id"), out));
            let key = p.id(|b, out| type_bridge_plan_builder_field(b, thing, field, out));
            let order = p
                .id(|b, out| type_bridge_plan_builder_input(b, p.text("order_id"), STRING, 0, out));
            p.ok(|b| type_bridge_plan_builder_compare(b, BINDING, key, EQUAL, INPUT, order));
            let roles = [
                FetchRole {
                    role: p.id(|b, out| type_bridge_plan_builder_role(b, ty, p.text("buyer"), out)),
                    required: 1,
                },
                FetchRole {
                    role: p.id(|b, out| type_bridge_plan_builder_role(b, ty, p.text("item"), out)),
                    required: u8::from(required),
                },
            ];
            p.ok(|b| type_bridge_plan_builder_fetch(b, thing, roles.as_ptr(), roles.len()));
            let value = p.id(|b, out| type_bridge_plan_builder_value_string(b, p.text("O1"), out));
            p.ok(|b| type_bridge_plan_builder_row(b, &value, 1));
            assert_eq!(
                p.build(&package).unwrap(),
                fixture.read_back(required).canonical_bytes().unwrap()
            );
            p.ok(|b| type_bridge_plan_builder_fetch(b, thing, ptr::null(), 0));
            assert_eq!(
                p.build(&package).unwrap_err().1,
                "query_plan_builder_fetch_once"
            );
        }
    }
    for fetch_first in [false, true] {
        let p = B::new();
        // SAFETY: live handles and valid output slots.
        unsafe {
            let ty =
                p.id(|b, out| type_bridge_plan_builder_type(b, RELATION, p.text("purchase"), out));
            let thing = p.id(|b, out| type_bridge_plan_builder_exact(b, ty, out));
            if fetch_first {
                p.ok(|b| type_bridge_plan_builder_fetch(b, thing, ptr::null(), 0));
            }
            p.ok(|b| type_bridge_plan_builder_count(b));
            if !fetch_first {
                p.ok(|b| type_bridge_plan_builder_fetch(b, thing, ptr::null(), 0));
            }
            assert_eq!(
                p.build(&package).unwrap_err().1,
                "query_plan_builder_terminal_conflict"
            );
        }
    }
}

#[test]
fn fetch_role_arrays_validate_pointer_ranges_and_boolean_flags() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let p = B::new();
    // SAFETY: invalid pointer/length pairs and overflow are rejected before reads.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_fetch(p.0, 0, ptr::null(), 1),
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            type_bridge_plan_builder_fetch(p.0, 0, ptr::dangling(), usize::MAX),
            TypeBridgeStatus::InvalidArgument
        );
        let ty = p.id(|b, out| type_bridge_plan_builder_type(b, RELATION, p.text("purchase"), out));
        let thing = p.id(|b, out| type_bridge_plan_builder_exact(b, ty, out));
        let role = p.id(|b, out| type_bridge_plan_builder_role(b, ty, p.text("buyer"), out));
        let roles = [FetchRole { role, required: 2 }];
        p.ok(|b| type_bridge_plan_builder_fetch(b, thing, roles.as_ptr(), roles.len()));
        assert_eq!(
            p.build(&package).unwrap_err().1,
            "c_plan_builder_constant_invalid"
        );
    }
}

#[test]
fn typed_constructor_poison_reaches_build_with_one_named_diagnostic() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let p = B::new();
    // SAFETY: the builder and all output slots remain live, and the two literals borrow no memory.
    unsafe {
        let poison = type_bridge_plan_long_sub(
            type_bridge_plan_long_literal(2),
            type_bridge_plan_long_literal(1),
        );
        assert_eq!(
            type_bridge_plan_long_set(p.0, 0, 0, poison),
            TypeBridgeStatus::InvalidArgument
        );
        let mut plan = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            type_bridge_plan_builder_build(p.0, package.handle, &mut plan, &mut diagnostics),
            TypeBridgeStatus::InvalidArgument
        );
        assert!(plan.is_null());
        let mut count = 0;
        assert_eq!(
            type_bridge_execution_diagnostics_count(diagnostics, &mut count),
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        let mut item = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            type_bridge_execution_diagnostics_get_v1(diagnostics, 0, item.as_mut_ptr()),
            TypeBridgeStatus::Ok
        );
        let item = item.assume_init();
        assert_eq!(
            item.category,
            TypeBridgeExecutionDiagnosticCategory::InvalidInput
        );
        assert_eq!(
            std::slice::from_raw_parts(item.code.data, item.code.length),
            b"c_plan_constructor_missing_builder"
        );
        assert_eq!(
            std::slice::from_raw_parts(item.message.data, item.message.length),
            b"tb_long_sub: no operand carried a builder"
        );
        assert_eq!(item.path_count, 0);
        type_bridge_execution_diagnostics_close(&mut diagnostics);
        assert!(diagnostics.is_null());
    }
}

#[test]
fn typed_fields_key_and_expression_match_explicit_core_bytes() {
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::v3::{ArithOp, Expr, PlanBuilder as CoreBuilder};
    use type_bridge_contract::query_plan::{InputColumnId, QueryOperand};
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let p = B::new();
    // SAFETY: all handles, views and output slots remain live for these calls.
    unsafe {
        let model = p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
        let key = p.id(|b, out| type_bridge_plan_builder_owns(b, model, p.text("item-id"), out));
        let stock = p.id(|b, out| type_bridge_plan_builder_owns(b, model, p.text("stock"), out));
        let item = p.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        p.ok(|b| {
            type_bridge_plan_string_key(b, item, key, type_bridge_plan_string_literal(p.text("I1")))
        });
        let value = type_bridge_plan_long_field(p.0, item, stock);
        p.ok(|b| {
            type_bridge_plan_require(
                b,
                type_bridge_plan_long_gt(value, type_bridge_plan_long_literal(0)),
            )
        });
        p.ok(|b| {
            type_bridge_plan_long_set(
                b,
                item,
                stock,
                type_bridge_plan_long_sub(value, type_bridge_plan_long_literal(1)),
            )
        });
    }
    let mut core = CoreBuilder::new();
    let model = TypeId::new(TypeKind::Entity, "item").unwrap();
    let item = core.exact(model.clone());
    let key = core.field(item, AttributeId::new("item-id").unwrap());
    core.input("item_key", ValueTypeTag::String, false);
    core.compare(
        QueryOperand::Binding { binding: key },
        ValueComparator::Equal,
        QueryOperand::Input {
            column: InputColumnId::new(0),
        },
    );
    let stock = core.field(item, AttributeId::new("stock").unwrap());
    core.input("item_stock_2", ValueTypeTag::Long, false);
    core.compare(
        QueryOperand::Binding { binding: stock },
        ValueComparator::Greater,
        QueryOperand::Input {
            column: InputColumnId::new(1),
        },
    );
    core.input("item_stock_3", ValueTypeTag::Long, false);
    let next = core.let_(
        "item_stock_4",
        Expr::Arith {
            op: ArithOp::Sub,
            lhs: Box::new(Expr::Operand(QueryOperand::Binding { binding: stock })),
            rhs: Box::new(Expr::Operand(QueryOperand::Input {
                column: InputColumnId::new(2),
            })),
        },
    );
    core.update(
        item,
        OwnsFactId::new(model, AttributeId::new("stock").unwrap()).unwrap(),
        QueryOperand::Binding { binding: next },
    );
    core.row(vec![
        Some(CanonicalValue::String(CanonicalString::new("I1").unwrap())),
        Some(CanonicalValue::Long(0)),
        Some(CanonicalValue::Long(1)),
    ]);
    let expected = core
        .build(
            fixture.authority.declared_schema(),
            fixture
                .authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        )
        .unwrap()
        .canonical_bytes()
        .unwrap();
    assert_eq!(p.build(&package).unwrap(), expected);
}

#[test]
fn typed_iid_match_forwards_core_bytes_and_utf8_faults() {
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::query_plan::v3::PlanBuilder as CoreBuilder;
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let p = B::new();
    let iid = "0x1234";
    // SAFETY: every builder, view and output slot remains live for the call.
    unsafe {
        let model = p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
        let item = p.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        p.ok(|b| type_bridge_plan_builder_by_iid(b, item, p.text(iid)));
        p.ok(|b| type_bridge_plan_builder_count(b));
    }
    let mut core = CoreBuilder::new();
    let item = core.exact(TypeId::new(TypeKind::Entity, "item").unwrap());
    core.by_iid(item, iid).count();
    let expected = core
        .build(
            fixture.authority.declared_schema(),
            fixture
                .authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        )
        .unwrap()
        .canonical_bytes()
        .unwrap();
    assert_eq!(p.build(&package).unwrap(), expected);
    let invalid = B::new();
    let bytes = [255u8];
    // SAFETY: the non-UTF8 view is readable; text validation records a typed fault.
    unsafe {
        invalid.ok(|b| {
            type_bridge_plan_builder_by_iid(
                b,
                0,
                TypeBridgeByteView {
                    data: bytes.as_ptr(),
                    length: bytes.len(),
                },
            )
        });
    }
    let (_, code) = invalid.build(&package).unwrap_err();
    assert_eq!(code, "c_plan_builder_value_invalid");
}

#[test]
fn typed_lazy_fields_ignore_getter_order_and_unused_carriers() {
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::PlanBuilder as CoreBuilder;
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let mut core = CoreBuilder::new();
    let account = core.exact(TypeId::new(TypeKind::Entity, "account").unwrap());
    let item = core.exact(TypeId::new(TypeKind::Entity, "item").unwrap());
    let balance = core.field_value(account, AttributeId::new("balance").unwrap());
    let price = core.field_value(item, AttributeId::new("price").unwrap());
    core.compare(
        QueryOperand::Binding { binding: balance },
        ValueComparator::GreaterOrEqual,
        QueryOperand::Binding { binding: price },
    )
    .count();
    let expected = core
        .build(
            fixture.authority.declared_schema(),
            fixture
                .authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        )
        .unwrap()
        .canonical_bytes()
        .unwrap();
    for mode in 0..4 {
        let p = B::new();
        // SAFETY: all carriers and views borrow this live builder through the consuming call.
        unsafe {
            let account_type =
                p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("account"), out));
            let item_type =
                p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
            let balance_field = p.id(|b, out| {
                type_bridge_plan_builder_owns(b, account_type, p.text("balance"), out)
            });
            let price_field =
                p.id(|b, out| type_bridge_plan_builder_owns(b, item_type, p.text("price"), out));
            let stock_field =
                p.id(|b, out| type_bridge_plan_builder_owns(b, item_type, p.text("stock"), out));
            let account = p.id(|b, out| type_bridge_plan_builder_exact(b, account_type, out));
            let item = p.id(|b, out| type_bridge_plan_builder_exact(b, item_type, out));
            let condition = match mode {
                0 => type_bridge_plan_long_ge(
                    type_bridge_plan_long_field(p.0, account, balance_field),
                    type_bridge_plan_long_field(p.0, item, price_field),
                ),
                1 | 3 => {
                    if mode == 3 {
                        let _unused = type_bridge_plan_long_field(p.0, item, stock_field);
                    }
                    let left = type_bridge_plan_long_field(p.0, account, balance_field);
                    let right = type_bridge_plan_long_field(p.0, item, price_field);
                    type_bridge_plan_long_ge(left, right)
                }
                _ => {
                    let right = type_bridge_plan_long_field(p.0, item, price_field);
                    let left = type_bridge_plan_long_field(p.0, account, balance_field);
                    type_bridge_plan_long_ge(left, right)
                }
            };
            p.ok(|b| type_bridge_plan_require(b, condition));
            p.ok(|b| type_bridge_plan_builder_count(b));
        }
        assert_eq!(
            p.build(&package).unwrap(),
            expected,
            "getter evaluation mode {mode}"
        );
    }
}

#[test]
fn typed_lazy_arithmetic_has_stable_depth_first_bytes_and_reuses_nodes() {
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::{ArithOp, Expr, PlanBuilder as CoreBuilder, Site};
    use type_bridge_contract::value::CanonicalValue;
    fn operation(
        core: &mut CoreBuilder,
        site: Site,
        left: BindingId,
        op: ArithOp,
        literal: i64,
    ) -> BindingId {
        let right = core.bind_value(site.clone(), CanonicalValue::Long(literal));
        core.let_value(
            site,
            Expr::Arith {
                op,
                lhs: Box::new(Expr::Operand(QueryOperand::Binding { binding: left })),
                rhs: Box::new(Expr::Operand(right)),
            },
        )
    }
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    for depth in [1, 3] {
        for mode in 0..5 {
            let mut core = CoreBuilder::new();
            let account = core.exact(TypeId::new(TypeKind::Entity, "account").unwrap());
            let item = core.exact(TypeId::new(TypeKind::Entity, "item").unwrap());
            let balance = core.field_value(account, AttributeId::new("balance").unwrap());
            let mut left = operation(
                &mut core,
                Site::new(account, "balance"),
                balance,
                ArithOp::Sub,
                1,
            );
            if depth == 3 {
                left = operation(
                    &mut core,
                    Site::new(account, "balance"),
                    left,
                    ArithOp::Mul,
                    2,
                );
                left = operation(
                    &mut core,
                    Site::new(account, "balance"),
                    left,
                    ArithOp::Add,
                    3,
                );
            }
            let right = if mode == 4 {
                left
            } else {
                let stock = core.field_value(item, AttributeId::new("stock").unwrap());
                operation(&mut core, Site::new(item, "stock"), stock, ArithOp::Sub, 2)
            };
            core.compare(
                QueryOperand::Binding { binding: left },
                ValueComparator::GreaterOrEqual,
                QueryOperand::Binding { binding: right },
            )
            .count();
            let expected = core
                .build(
                    fixture.authority.declared_schema(),
                    fixture
                        .authority
                        .managed_state()
                        .managed_semantic_schema()
                        .clone(),
                )
                .unwrap()
                .canonical_bytes()
                .unwrap();
            let p = B::new();
            // SAFETY: all carriers, literals and arena nodes borrow this live builder through consumption.
            unsafe {
                let account_type =
                    p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("account"), out));
                let item_type =
                    p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
                let balance = p.id(|b, out| {
                    type_bridge_plan_builder_owns(b, account_type, p.text("balance"), out)
                });
                let stock = p
                    .id(|b, out| type_bridge_plan_builder_owns(b, item_type, p.text("stock"), out));
                let price = p
                    .id(|b, out| type_bridge_plan_builder_owns(b, item_type, p.text("price"), out));
                let account = p.id(|b, out| type_bridge_plan_builder_exact(b, account_type, out));
                let item = p.id(|b, out| type_bridge_plan_builder_exact(b, item_type, out));
                let left = || {
                    let mut expression = type_bridge_plan_long_sub(
                        type_bridge_plan_long_field(p.0, account, balance),
                        type_bridge_plan_long_literal(1),
                    );
                    if depth == 3 {
                        expression =
                            type_bridge_plan_long_mul(expression, type_bridge_plan_long_literal(2));
                        expression =
                            type_bridge_plan_long_add(expression, type_bridge_plan_long_literal(3));
                    }
                    expression
                };
                let right = || {
                    type_bridge_plan_long_sub(
                        type_bridge_plan_long_field(p.0, item, stock),
                        type_bridge_plan_long_literal(2),
                    )
                };
                let condition = match mode {
                    0 => type_bridge_plan_long_ge(left(), right()),
                    1 | 3 => {
                        if mode == 3 {
                            let _unused = type_bridge_plan_long_sub(
                                type_bridge_plan_long_field(p.0, item, price),
                                type_bridge_plan_long_literal(999),
                            );
                        }
                        let a = left();
                        let b = right();
                        type_bridge_plan_long_ge(a, b)
                    }
                    2 => {
                        let b = right();
                        let a = left();
                        type_bridge_plan_long_ge(a, b)
                    }
                    _ => {
                        let a = left();
                        type_bridge_plan_long_ge(a, a)
                    }
                };
                p.ok(|b| type_bridge_plan_require(b, condition));
                p.ok(|b| type_bridge_plan_builder_count(b));
            }
            assert_eq!(
                p.build(&package).unwrap(),
                expected,
                "depth={depth}, creation mode={mode}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn compiled_lazy_c_and_cpp_expressions_match_core_in_every_creation_order() {
    let Some(library) = native_library() else {
        return;
    };
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::{ArithOp, Expr, PlanBuilder as CoreBuilder, Site};
    use type_bridge_contract::value::CanonicalValue;
    let fixture = purchase_package();
    let mut core = CoreBuilder::new();
    let account = core.exact(TypeId::new(TypeKind::Entity, "account").unwrap());
    let item = core.exact(TypeId::new(TypeKind::Entity, "item").unwrap());
    let mut left = core.field_value(account, AttributeId::new("balance").unwrap());
    for (op, value) in [(ArithOp::Sub, 1), (ArithOp::Mul, 2), (ArithOp::Add, 3)] {
        let site = Site::new(account, "balance");
        let right = core.bind_value(site.clone(), CanonicalValue::Long(value));
        left = core.let_value(
            site,
            Expr::Arith {
                op,
                lhs: Box::new(Expr::Operand(QueryOperand::Binding { binding: left })),
                rhs: Box::new(Expr::Operand(right)),
            },
        );
    }
    let stock = core.field_value(item, AttributeId::new("stock").unwrap());
    let decrement = core.bind_value(Site::new(item, "stock"), CanonicalValue::Long(2));
    let right = core.let_value(
        Site::new(item, "stock"),
        Expr::Arith {
            op: ArithOp::Sub,
            lhs: Box::new(Expr::Operand(QueryOperand::Binding { binding: stock })),
            rhs: Box::new(Expr::Operand(decrement)),
        },
    );
    core.compare(
        QueryOperand::Binding { binding: left },
        ValueComparator::GreaterOrEqual,
        QueryOperand::Binding { binding: right },
    )
    .count();
    let expected = core
        .build(
            fixture.authority.declared_schema(),
            fixture
                .authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        )
        .unwrap()
        .canonical_bytes()
        .unwrap();
    let expected = expected
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let unset_expected = [false, true].map(|remove| {
        let mut core = CoreBuilder::new();
        core.exact(TypeId::new(TypeKind::Entity, "account").unwrap());
        let item = core.exact(TypeId::new(TypeKind::Entity, "item").unwrap());
        if remove {
            core.delete_field(
                item,
                type_bridge_contract::schema::OwnsFactId::new(
                    TypeId::new(TypeKind::Entity, "item").unwrap(),
                    AttributeId::new("stock").unwrap(),
                )
                .unwrap(),
            );
        } else {
            core.count();
        }
        core.build(
            fixture.authority.declared_schema(),
            fixture
                .authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        )
        .unwrap()
        .canonical_bytes()
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
    });
    let create_expected = (0..5)
        .map(|case| {
            let mut core = CoreBuilder::new();
            let relation = matches!(case, 2 | 3);
            let model_id = TypeId::new(
                if relation {
                    TypeKind::Relation
                } else {
                    TypeKind::Entity
                },
                if relation { "purchase" } else { "item" },
            )
            .unwrap();
            let model = &fixture.projection.models()[&model_id];
            let players = relation.then(|| {
                (
                    core.exact(TypeId::new(TypeKind::Entity, "account").unwrap()),
                    core.exact(TypeId::new(TypeKind::Entity, "item").unwrap()),
                )
            });
            let thing = if matches!(case, 1 | 3) {
                core.put(model_id)
            } else {
                core.insert(model_id)
            };
            for create in model.create().fields() {
                let field = &model.query_tokens().fields()[create.token()];
                let attribute = field.id().attribute().label().as_str();
                if (case == 1 && attribute == "stock") || (case == 4 && attribute != "price") {
                    continue;
                }
                let value = match attribute {
                    "item-id" => CanonicalValue::String(
                        type_bridge_contract::value::CanonicalString::new("I1").unwrap(),
                    ),
                    "order-id" => CanonicalValue::String(
                        type_bridge_contract::value::CanonicalString::new("O1").unwrap(),
                    ),
                    "price" => CanonicalValue::Long(3),
                    "stock" => CanonicalValue::Long(0),
                    _ => unreachable!("fixture attribute"),
                };
                let value = core.bind_value(Site::new(thing, attribute), value);
                core.thing_field(thing, field.id().clone(), value);
            }
            if let Some((buyer, item)) = players {
                for role in model.create().roles().keys() {
                    core.thing_player(
                        thing,
                        role.clone(),
                        if role.label().as_str() == "buyer" {
                            buyer
                        } else {
                            item
                        },
                    );
                }
            }
            core.build(
                fixture.authority.declared_schema(),
                fixture
                    .authority
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
            )
            .unwrap()
            .canonical_bytes()
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
        })
        .collect::<Vec<_>>();
    let purchase =
        &fixture.projection.models()[&TypeId::new(TypeKind::Relation, "purchase").unwrap()];
    let purchase_players = purchase
        .query_tokens()
        .roles()
        .values()
        .map(|role| {
            let member = role.role().label().as_str();
            let player = if member == "buyer" { "account" } else { "item" };
            if role.multiplicity().container()
                == type_bridge_contract::projection::ProjectedContainer::Scalar
            {
                format!("created.{member} = {player};")
            } else {
                format!("created.{member} = &{player}; created.{member}_count = 1u;")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let stage = TempDirectory::new("compiled_lazy_order");
    for (path, bytes) in fixture.package.files() {
        let target = stage.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let foreign = purchase_package::purchase_package_with_prefix("other");
    let foreign_root = stage.path().join("foreign");
    for (path, bytes) in foreign.package.files() {
        let target = foreign_root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let main = stage.path().join("lazy.c");
    fs::write(&main, r#"
#include <stdio.h>
#include <stdlib.h>
#include <shop/models.h>
#include <other/models.h>
#ifdef __cplusplus
extern "C" {
#endif
void *__real_malloc(size_t); void *__real_calloc(size_t, size_t); void *__real_realloc(void *, size_t);
static size_t generated_allocations = 0u;
void *__wrap_malloc(size_t n) { ++generated_allocations; return __real_malloc(n); }
void *__wrap_calloc(size_t n, size_t size) { ++generated_allocations; return __real_calloc(n, size); }
void *__wrap_realloc(void *p, size_t n) { ++generated_allocations; return __real_realloc(p, n); }
#ifdef __cplusplus
}
#endif
#define OK(call) do { type_bridge_status_t status = (call); if (status != TYPE_BRIDGE_STATUS_OK) { fprintf(stderr, "line %d status %d\n", __LINE__, (int)status); return 2; } } while (0)
static tb_long_t left(type_bridge_plan_builder_t *p, shop_account_handle_t account) {
  return tb_long_add(tb_long_mul(tb_long_sub(shop_account_plan_balance(p, account), tb_long(1)), tb_long(2)), tb_long(3));
}
static tb_long_t right(type_bridge_plan_builder_t *p, shop_item_handle_t item) {
  return tb_long_sub(shop_item_plan_stock(p, item), tb_long(2));
}
int main(int argc, char **argv) {
  type_bridge_plan_builder_t *p = NULL;
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_plan_t *plan = NULL;
  shop_account_handle_t account; shop_item_handle_t item;
  shop_account_match_t accounts; shop_item_match_t items;
  tb_cond_t condition; size_t length = 0u, index; uint8_t *bytes;
  const int mode = argc > 1 ? atoi(argv[1]) : 0;
  memset(&accounts, 0, sizeof(accounts)); memset(&items, 0, sizeof(items));
  OK(shop_schema_package_open(&package, &package_diagnostics));
  OK(shop_plan_open(NULL, &p, &diagnostics));
  if (mode >= 12 && mode <= 17) {
    if (mode == 14 || mode == 15) {
      shop_purchase_insert_t created; shop_purchase_handle_t purchase;
      memset(&created, 0, sizeof(created));
      OK(shop_account_match_by(p, &accounts, &account)); OK(shop_item_match_by(p, &items, &item));
      created.orderzhid = "O1"; created.has_orderzhid = true;
      @PURCHASE_PLAYERS@
      OK(shop_purchase_plan_create_handle(p, mode == 15, &created, &purchase));
    } else {
      shop_item_insert_t created; shop_item_handle_t inserted;
      memset(&created, 0, sizeof(created));
      created.price = 3; created.has_price = true;
      if (mode != 17) { created.itemzhid = mode == 16 ? NULL : "I1"; created.has_itemzhid = true; }
      if (mode == 12) { created.stock = 0; created.has_stock = true; }
      if (mode == 16) {
        type_bridge_execution_diagnostic_view_v1_t view;
        if (shop_item_plan_create_handle(p, false, &created, &inserted) == TYPE_BRIDGE_STATUS_OK) return 13;
        if (type_bridge_plan_builder_build(p, package, &plan, &diagnostics) == TYPE_BRIDGE_STATUS_OK || diagnostics == NULL) return 14;
        OK(type_bridge_execution_diagnostics_get_v1(diagnostics, 0u, &view)); fwrite(view.code.data, 1u, view.code.length, stdout);
        OK(type_bridge_execution_diagnostics_close(&diagnostics)); type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
      }
      OK(shop_item_plan_create_handle(p, mode == 13, &created, &inserted));
    }
    goto emit_plan;
  }
  if (mode == 9) {
    type_bridge_plan_builder_t *other = NULL; shop_item_handle_t foreign;
    type_bridge_status_t status;
    OK(shop_plan_open(NULL, &other, &diagnostics)); OK(shop_item_match_by(other, &items, &foreign));
    items.stock = shop_item_plan_stock(other, foreign);
    status = shop_item_match_by(p, &items, (shop_item_handle_t *)(void *)other);
    if (status != TYPE_BRIDGE_STATUS_INVALID_ARGUMENT) return 10;
    OK(type_bridge_plan_builder_count(other));
    OK(type_bridge_plan_builder_build(other, package, &plan, &diagnostics));
    type_bridge_plan_free(&plan); type_bridge_plan_builder_free(&other);
    fputs("foreign_builder_alias_refused", stdout);
    type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
  }
  if (mode == 10) {
    union { shop_item_handle_t alignment; uint8_t bytes[sizeof(shop_item_handle_t)]; } literal, before;
    type_bridge_byte_view_t view; type_bridge_status_t status;
    memset(&literal, 0xa5, sizeof(literal)); literal.bytes[0] = 'I'; literal.bytes[1] = '1';
    memcpy(&before, &literal, sizeof(literal)); view.data = literal.bytes; view.length = 2u;
    items.itemzhid = tb_string(view);
    status = shop_item_match_by(p, &items, (shop_item_handle_t *)(void *)literal.bytes);
    if (status != TYPE_BRIDGE_STATUS_INVALID_ARGUMENT || memcmp(&before, &literal, sizeof(literal)) != 0) return 11;
    fputs("literal_alias_refused", stdout);
    type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
  }
  if (mode == 11) {
    union { shop_item_handle_t alignment; char bytes[sizeof(shop_item_handle_t) + 8u]; } iid, before;
    type_bridge_status_t status;
    memset(&iid, 0xa5, sizeof(iid)); memcpy(iid.bytes, "0x123456", 9u); memcpy(&before, &iid, sizeof(iid));
    status = shop_item_by_iid(p, iid.bytes, (shop_item_handle_t *)(void *)(iid.bytes + 8u));
    if (status != TYPE_BRIDGE_STATUS_INVALID_ARGUMENT || memcmp(&before, &iid, sizeof(iid)) != 0) return 12;
    fputs("terminator_alias_refused", stdout);
    type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
  }
  if (mode == 8) {
    shop_item_handle_t bulk; uint32_t model = 0u; size_t before;
    shop_item_values_t *rows = (shop_item_values_t *)calloc(10000u, sizeof(*rows));
    char *names = (char *)malloc(10000u * 16u);
    type_bridge_byte_view_t opener = {(const uint8_t *)"shop_plan_open", 14u};
    if (rows == NULL || names == NULL) return 8;
    memset(&bulk, 0, sizeof(bulk)); bulk.recording = p; bulk.model_ordinal = @ITEM_ORDINAL@u;
    OK(type_bridge_plan_builder_type_token(p, &shop_projected_model_token_@ITEM_ORDINAL@, opener, &model));
    OK(type_bridge_plan_builder_insert(p, model, &bulk.binding));
    for (index = 0u; index < 10000u; ++index) {
      (void)snprintf(names + index * 16u, 16u, "I%zu", index);
      rows[index].itemzhid = names + index * 16u; rows[index].has_itemzhid = true;
      rows[index].price = 3; rows[index].has_price = true;
      rows[index].stock = 10; rows[index].has_stock = true;
    }
    before = generated_allocations;
    OK(shop_item_plan_bind_rows(p, bulk, rows, 10000u));
    if (generated_allocations != before) return 9;
    free(names); free(rows);
    fputs("zero_generated_allocations", stdout);
    type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
  }
  if (mode == 4 || mode == 5) {
    type_bridge_status_t status; type_bridge_execution_diagnostic_view_v1_t view;
    if (mode == 4) { other_item_handle_t h; status = other_item_subtypes(p, &h); }
    else { shop_item_handle_t h; type_bridge_plan_builder_free(&p); OK(type_bridge_plan_builder_new(&p)); status = shop_item_subtypes(p, &h); }
    if (status == TYPE_BRIDGE_STATUS_OK) return 5;
    if (type_bridge_plan_builder_build(p, package, &plan, &diagnostics) == TYPE_BRIDGE_STATUS_OK || diagnostics == NULL) return 6;
    OK(type_bridge_execution_diagnostics_get_v1(diagnostics, 0u, &view));
    fwrite(view.code.data, 1u, view.code.length, stdout);
    OK(type_bridge_execution_diagnostics_close(&diagnostics));
    type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
  }

  OK(shop_account_match_by(p, &accounts, &account));
  OK(shop_item_match_by(p, &items, &item));
  if (mode == 6 || mode == 7) {
    shop_item_unset_t fields; memset(&fields, 0, sizeof(fields));
    OK(shop_item_unset(p, item, &fields));
    if (mode == 7) {
#ifdef __cplusplus
      fields.stock = true;
#else
      fields = (shop_item_unset_t){ .stock = true };
#endif
      OK(shop_item_unset(p, item, &fields));
    }
    else { OK(type_bridge_plan_builder_count(p)); }
  } else {
  if (mode == 1) { tb_long_t a = left(p, account); tb_long_t b = right(p, item); condition = tb_long_ge(a, b); }
  else if (mode == 2) { tb_long_t b = right(p, item); tb_long_t a = left(p, account); condition = tb_long_ge(a, b); }
  else { if (mode == 3) { (void)tb_long_sub(shop_item_plan_price(p, item), tb_long(999)); } condition = tb_long_ge(left(p, account), right(p, item)); }
  OK(type_bridge_plan_require(p, condition)); OK(type_bridge_plan_builder_count(p));
  }
emit_plan:
  OK(type_bridge_plan_builder_build(p, package, &plan, &diagnostics));
  if (type_bridge_plan_v3_canonical_bytes(plan, NULL, 0u, &length) != TYPE_BRIDGE_STATUS_RESOURCE_LIMIT || length == 0u) return 4;
  bytes = (uint8_t *)malloc(length); if (bytes == NULL) return 3;
  OK(type_bridge_plan_v3_canonical_bytes(plan, bytes, length, &length));
  for (index = 0; index < length; ++index) printf("%02x", bytes[index]);
  free(bytes); type_bridge_plan_free(&plan); type_bridge_plan_builder_free(&p);
  OK(type_bridge_schema_package_close(&package)); return 0;
}
"#.replace("@ITEM_ORDINAL@", &item_model_token(&fixture).ordinal.to_string()).replace("@PURCHASE_PLAYERS@", &purchase_players)).unwrap();
    let include = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("include");
    let object = stage.path().join("models.o");
    let output = Command::new("cc")
        .args(["-std=c17", "-I"])
        .arg(&include)
        .arg("-I")
        .arg(stage.path().join("include"))
        .arg("-c")
        .arg(stage.path().join("src/models.c"))
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let foreign_object = stage.path().join("foreign.o");
    let compiled = Command::new("cc")
        .args(["-std=c17", "-I"])
        .arg(&include)
        .arg("-I")
        .arg(foreign_root.join("include"))
        .arg("-c")
        .arg(foreign_root.join("src/models.c"))
        .arg("-o")
        .arg(&foreign_object)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    for (compiler, language, standard) in [
        ("gcc", "c", "c17"),
        ("clang", "c", "c17"),
        ("g++", "c++", "c++17"),
        ("clang++", "c++", "c++17"),
    ] {
        let executable = stage.path().join(compiler);
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
            ])
            .arg(&include)
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg("-I")
            .arg(foreign_root.join("include"))
            .arg(&main)
            .args(["-x", "none"])
            .arg(&object)
            .arg(&foreign_object)
            .arg(&library)
            .args([
                "-Wl,--wrap=malloc",
                "-Wl,--wrap=calloc",
                "-Wl,--wrap=realloc",
            ])
            .arg(format!(
                "-Wl,-rpath,{}",
                library.parent().unwrap().display()
            ))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for mode in 0..18 {
            let output = Command::new(&executable)
                .arg(mode.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{compiler}, mode={mode}, exit={:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                if mode == 4 {
                    "generated_token_package_mismatch"
                } else if mode == 5 {
                    "plan_context_missing"
                } else if mode == 8 {
                    "zero_generated_allocations"
                } else if mode == 9 {
                    "foreign_builder_alias_refused"
                } else if mode == 10 {
                    "literal_alias_refused"
                } else if mode == 11 {
                    "terminator_alias_refused"
                } else if (12..16).contains(&mode) {
                    &create_expected[mode - 12]
                } else if mode == 16 {
                    "c_plan_builder_value_invalid"
                } else if mode == 17 {
                    &create_expected[4]
                } else if mode >= 6 {
                    &unset_expected[mode - 6]
                } else {
                    &expected
                },
                "{compiler}, creation mode={mode}"
            );
        }
    }
}

fn item_model_token(fixture: &PurchasePackage) -> type_bridge_c::TypeBridgeProjectedTokenV1 {
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::projection::ProjectedTokenIdentity;
    type_bridge_c::TypeBridgeProjectedTokenV1 {
        struct_size: size_of::<type_bridge_c::TypeBridgeProjectedTokenV1>() as u32,
        version: 1,
        kind: 1,
        ordinal: fixture
            .projection
            .projected_token_ordinal(&ProjectedTokenIdentity::Model(
                TypeId::new(TypeKind::Entity, "item").unwrap(),
            ))
            .unwrap(),
        projection_digest: fixture
            .projection
            .projection_fingerprint()
            .as_fingerprint()
            .digest()
            .bytes(),
        reserved: [0; 4],
    }
}

#[test]
fn typed_context_retains_package_checks_brand_and_names_the_opener() {
    let fixture = purchase_package();
    let token = item_model_token(&fixture);
    let package = OpenPackage::new(&fixture);
    let mut pointer = ptr::null_mut();
    // SAFETY: package and output are live and disjoint; defaults need no options.
    assert_eq!(
        unsafe {
            type_bridge_plan_builder_new_with_package(package.handle, ptr::null(), &mut pointer)
        },
        TypeBridgeStatus::Ok
    );
    let bound = B(pointer);
    drop(package);
    // SAFETY: the recording owns its package Arc after the caller package closes.
    unsafe {
        let model = bound.id(|b, out| {
            type_bridge_plan_builder_type_token(b, &token, bound.text("shop_plan_open"), out)
        });
        bound.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        bound.ok(|b| type_bridge_plan_builder_count(b));
    }
    let package = OpenPackage::new(&fixture);
    assert!(bound.build(&package).is_ok());
    let mut foreign = token;
    foreign.projection_digest[0] ^= 1;
    let mut out = 99;
    // SAFETY: the foreign token is readable; identity disagreement is a content refusal.
    assert_ne!(
        unsafe {
            type_bridge_plan_builder_type_token(
                bound.0,
                &foreign,
                bound.text("shop_plan_open"),
                &mut out,
            )
        },
        TypeBridgeStatus::Ok
    );
    assert_eq!(out, 0);
    assert_eq!(
        bound.build(&package).unwrap_err().1,
        "generated_token_package_mismatch"
    );

    let raw = B::new();
    // SAFETY: raw is a live unbound builder; all arguments/output are readable/writable.
    assert_eq!(
        unsafe {
            type_bridge_plan_builder_type_token(raw.0, &token, raw.text("shop_plan_open"), &mut out)
        },
        TypeBridgeStatus::InvalidArgument
    );
    let mut plan = ptr::null_mut();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: every handle and separate output remains live for these calls.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_build(raw.0, package.handle, &mut plan, &mut diagnostics),
            TypeBridgeStatus::InvalidArgument
        );
        let mut view = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
        assert_eq!(
            type_bridge_execution_diagnostics_get_v1(diagnostics, 0, view.as_mut_ptr()),
            TypeBridgeStatus::Ok
        );
        let view = view.assume_init();
        assert_eq!(
            std::str::from_utf8(std::slice::from_raw_parts(view.code.data, view.code.length))
                .unwrap(),
            "plan_context_missing"
        );
        assert_eq!(view.path_count, 1);
        let mut path = std::mem::MaybeUninit::<
            type_bridge_c::TypeBridgeExecutionDiagnosticPathViewV1,
        >::uninit();
        assert_eq!(
            type_bridge_execution_diagnostics_path_get_v1(diagnostics, 0, 0, path.as_mut_ptr()),
            TypeBridgeStatus::Ok
        );
        let path = path.assume_init();
        assert_eq!(
            path.kind,
            type_bridge_c::TypeBridgeExecutionDiagnosticPathKind::Argument
        );
        assert_eq!(
            std::str::from_utf8(std::slice::from_raw_parts(
                path.primary.data,
                path.primary.length
            ))
            .unwrap(),
            "shop_plan_open"
        );
        type_bridge_execution_diagnostics_close(&mut diagnostics);
    }
}

#[test]
fn typed_context_snapshots_limits_and_fences_nested_option_ranges() {
    use type_bridge_c::{TypeBridgePlanOptionsV1, TypeBridgeQueryLimitOverrideV1};
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let mut limits = [
        TypeBridgeQueryLimitOverrideV1 {
            name: b"canonical_bytes".as_ptr(),
            name_length: b"canonical_bytes".len(),
            value: 512,
        },
        TypeBridgeQueryLimitOverrideV1 {
            name: b"canonical_string_bytes".as_ptr(),
            name_length: b"canonical_string_bytes".len(),
            value: 512,
        },
    ];
    let mut options = TypeBridgePlanOptionsV1 {
        struct_size: size_of::<TypeBridgePlanOptionsV1>() as u32,
        version: 1,
        query_limits: limits.as_ptr(),
        query_limit_count: limits.len(),
        reserved: [0; 4],
    };
    let mut pointer = ptr::null_mut();
    // SAFETY: the complete options graph is live and separate from the output.
    assert_eq!(
        unsafe {
            type_bridge_plan_builder_new_with_package(package.handle, &options, &mut pointer)
        },
        TypeBridgeStatus::Ok
    );
    let p = B(pointer);
    limits[0].value = 16 * 1024 * 1024;
    limits[1].value = 1024 * 1024;
    assert_eq!(limits[0].value, 16 * 1024 * 1024);
    // SAFETY: all identity views and slots remain live for the primitive escape-hatch calls.
    unsafe {
        let model = p.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, p.text("item"), out));
        p.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        p.ok(|b| type_bridge_plan_builder_count(b));
    }
    assert_eq!(p.build(&package).unwrap_err().1, "canonical_json_too_large");
    options.reserved[0] = u64::MAX;
    let alias = options.reserved.as_mut_ptr().cast::<*mut Builder>();
    // SAFETY: aliasing output is deliberately rejected before options are changed or parsed.
    assert_eq!(
        unsafe { type_bridge_plan_builder_new_with_package(package.handle, &options, alias) },
        TypeBridgeStatus::InvalidArgument
    );
    assert_eq!(options.reserved[0], u64::MAX);
    let mut name = *b"canonical_bytes";
    let entry = TypeBridgeQueryLimitOverrideV1 {
        name: name.as_ptr(),
        name_length: name.len(),
        value: 512,
    };
    options.query_limits = &entry;
    options.query_limit_count = 1;
    options.reserved = [0; 4];
    let before = name;
    // SAFETY: the writable name buffer overlaps the output slot; no mutation is admitted.
    assert_eq!(
        unsafe {
            type_bridge_plan_builder_new_with_package(
                package.handle,
                &options,
                name.as_mut_ptr().cast(),
            )
        },
        TypeBridgeStatus::InvalidArgument
    );
    assert_eq!(name, before);
}

#[test]
fn model_metadata_is_reused_and_field_getters_allocate_and_record_nothing() {
    let fixture = purchase_package();
    let token = item_model_token(&fixture);
    let package = OpenPackage::new(&fixture);
    let mut pointer = ptr::null_mut();
    // SAFETY: package and outputs are live and disjoint.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_new_with_package(package.handle, ptr::null(), &mut pointer),
            TypeBridgeStatus::Ok
        );
    }
    let builder = B(pointer);
    let opener = builder.text("shop_plan_open");
    // SAFETY: all handles, descriptors and outputs live for the calls.
    unsafe {
        let model =
            builder.id(|b, out| type_bridge_plan_builder_type_token(b, &token, opener, out));
        // Admit descending ordinals: filling a lower hole must preserve higher cache entries.
        let mut admitted = Vec::new();
        for ordinal in (0..fixture.projection.models().len() as u32).rev() {
            let other = type_bridge_c::TypeBridgeProjectedTokenV1 { ordinal, ..token };
            let id =
                builder.id(|b, out| type_bridge_plan_builder_type_token(b, &other, opener, out));
            admitted.push((other, id));
        }
        for (other, id) in &admitted {
            assert_eq!(
                builder.id(|b, out| type_bridge_plan_builder_type_token(b, other, opener, out)),
                *id
            );
        }
        let owner = builder.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        let field = type_bridge_plan_builder_model_field(builder.0, token.ordinal, 0);
        assert_ne!(field, u32::MAX);
        builder.ok(|b| type_bridge_plan_builder_count(b));
        let before = builder.build(&package).unwrap();
        ALLOCATIONS.with(|count| count.set(Some(0)));
        for _ in 0..10_000 {
            let mut repeated = u32::MAX;
            assert_eq!(
                type_bridge_plan_builder_type_token(builder.0, &token, opener, &mut repeated),
                TypeBridgeStatus::Ok
            );
            assert_eq!(repeated, model);
            assert_eq!(
                type_bridge_plan_builder_model_field(builder.0, token.ordinal, 0),
                field
            );
            std::hint::black_box(type_bridge_plan_long_field(builder.0, owner, field));
        }
        let allocations = ALLOCATIONS.with(|count| count.replace(None).unwrap());
        assert_eq!(
            allocations, 0,
            "cache hits and unused getters allocate nothing"
        );
        assert_eq!(
            builder.build(&package).unwrap(),
            before,
            "unused getters record nothing"
        );
    }
}

#[cfg(unix)]
#[test]
fn compiled_nominal_union_role_edits_copy_borrowed_arrays_and_refuse_unknown_tags() {
    let Some(library) = native_library() else {
        return;
    };
    use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
    use type_bridge_contract::query_plan::v3::PlanBuilder as CoreBuilder;
    let fixture = purchase_package::package_from_source(
        r#"format: typebridge.schema/v2
entities:
  a: {}
  b: {}
relations:
  group:
    relates:
      member: {card: {min: 0, max: 3}}
plays:
  a:
    group: {member: {}}
  b:
    group: {member: {}}
"#,
        "shop",
    );
    let expected = (0..3)
        .map(|mode| {
            let mut core = CoreBuilder::new();
            let group = core.subtypes(TypeId::new(TypeKind::Relation, "group").unwrap());
            let a = core.subtypes(TypeId::new(TypeKind::Entity, "a").unwrap());
            let b = core.subtypes(TypeId::new(TypeKind::Entity, "b").unwrap());
            if mode == 0 {
                core.count();
            } else {
                for player in [a, b] {
                    let role = RoleId::new("group", "member").unwrap();
                    if mode == 1 {
                        core.link(group, role, player);
                    } else {
                        core.delete_player(group, role, player);
                    }
                }
            }
            core.build(
                fixture.authority.declared_schema(),
                fixture
                    .authority
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
            )
            .unwrap()
            .canonical_bytes()
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
        })
        .collect::<Vec<_>>();
    let create_expected = {
        let mut core = CoreBuilder::new();
        core.subtypes(TypeId::new(TypeKind::Relation, "group").unwrap());
        let a = core.subtypes(TypeId::new(TypeKind::Entity, "a").unwrap());
        let b = core.subtypes(TypeId::new(TypeKind::Entity, "b").unwrap());
        let group = core.insert(TypeId::new(TypeKind::Relation, "group").unwrap());
        for player in [a, b] {
            core.thing_player(group, RoleId::new("group", "member").unwrap(), player);
        }
        core.build(
            fixture.authority.declared_schema(),
            fixture
                .authority
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        )
        .unwrap()
        .canonical_bytes()
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
    };
    let stage = TempDirectory::new("compiled_role_edits");
    for (path, bytes) in fixture.package.files() {
        let target = stage.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let main = stage.path().join("roles.c");
    fs::write(&main, r#"
#include <stdio.h>
#include <stdlib.h>
#include <shop/models.h>
#define OK(call) do { type_bridge_status_t status = (call); if (status != TYPE_BRIDGE_STATUS_OK) { fprintf(stderr, "line %d status %d\n", __LINE__, (int)status); return 2; } } while (0)
int main(int argc, char **argv) {
  type_bridge_plan_builder_t *p = NULL; type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL; type_bridge_plan_t *plan = NULL;
  shop_group_handle_t group; shop_a_handle_t a; shop_b_handle_t b;
  shop_group_link_t links; shop_group_unlink_t unlinks;
  shop_group_plan_member_player_t *players;
  type_bridge_status_t status; size_t length = 0u, index; uint8_t *bytes;
  const int mode = argc > 1 ? atoi(argv[1]) : 0;
  OK(shop_schema_package_open(&package, &package_diagnostics)); OK(shop_plan_open(NULL, &p, &diagnostics));
  OK(shop_group_subtypes(p, &group)); OK(shop_a_subtypes(p, &a)); OK(shop_b_subtypes(p, &b));
  memset(&links, 0, sizeof(links)); memset(&unlinks, 0, sizeof(unlinks));
  OK(shop_group_link(p, group, &links)); OK(shop_group_unlink(p, group, &unlinks));
  if (mode == 0) { OK(type_bridge_plan_builder_count(p)); }
  else {
    players = (shop_group_plan_member_player_t *)calloc(2u, sizeof(*players)); if (players == NULL) return 3;
    players[0].kind = shop_group_plan_member_player_a; players[0].value.a = a;
    players[1].kind = shop_group_plan_member_player_b; players[1].value.b = b;
    if (mode == 4) memset(players, 0, 2u * sizeof(*players));
    if (mode == 3) players[0].kind = 999u;
    links.member = players; links.member_count = 2u;
    unlinks.member = players; unlinks.member_count = 2u;
    if (mode >= 5) {
      shop_group_insert_t created; shop_group_handle_t inserted;
      memset(&created, 0, sizeof(created)); created.member = players; created.member_count = 2u;
      if (mode == 6) {
        shop_group_plan_member_player_t before[2]; memcpy(before, players, sizeof(before));
        status = shop_group_plan_create_handle(p, false, &created, (shop_group_handle_t *)(void *)players);
        if (status != TYPE_BRIDGE_STATUS_INVALID_ARGUMENT || memcmp(before, players, sizeof(before)) != 0) return 9;
        status = TYPE_BRIDGE_STATUS_OK;
      } else if (mode == 7) {
        type_bridge_plan_builder_t *other = NULL; shop_a_handle_t foreign;
        OK(shop_plan_open(NULL, &other, &diagnostics)); OK(shop_a_subtypes(other, &foreign));
        players[0].value.a = foreign;
        status = shop_group_plan_create_handle(p, false, &created, (shop_group_handle_t *)(void *)other);
        if (status != TYPE_BRIDGE_STATUS_INVALID_ARGUMENT) return 10;
        OK(type_bridge_plan_builder_count(other)); OK(type_bridge_plan_builder_build(other, package, &plan, &diagnostics));
        type_bridge_plan_free(&plan); type_bridge_plan_builder_free(&other); status = TYPE_BRIDGE_STATUS_OK;
      } else { status = shop_group_plan_create_handle(p, false, &created, &inserted); }
    } else { status = mode == 2 ? shop_group_unlink(p, group, &unlinks) : shop_group_link(p, group, &links); }
    free(players);
    if (mode == 3) {
      type_bridge_execution_diagnostic_view_v1_t view;
      if (status == TYPE_BRIDGE_STATUS_OK) return 4;
      if (type_bridge_plan_builder_build(p, package, &plan, &diagnostics) == TYPE_BRIDGE_STATUS_OK || diagnostics == NULL) return 5;
      OK(type_bridge_execution_diagnostics_get_v1(diagnostics, 0u, &view)); fwrite(view.code.data, 1u, view.code.length, stdout);
      OK(type_bridge_execution_diagnostics_close(&diagnostics)); type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
    }
    if (status != TYPE_BRIDGE_STATUS_OK) return 6;
    if (mode == 4 || mode == 6 || mode == 7) { OK(type_bridge_plan_builder_count(p)); }
  }
  OK(type_bridge_plan_builder_build(p, package, &plan, &diagnostics));
  if (type_bridge_plan_v3_canonical_bytes(plan, NULL, 0u, &length) != TYPE_BRIDGE_STATUS_RESOURCE_LIMIT || length == 0u) return 7;
  bytes = (uint8_t *)malloc(length); if (bytes == NULL) return 8;
  OK(type_bridge_plan_v3_canonical_bytes(plan, bytes, length, &length));
  for (index = 0; index < length; ++index) printf("%02x", bytes[index]);
  free(bytes); type_bridge_plan_free(&plan); type_bridge_plan_builder_free(&p); OK(type_bridge_schema_package_close(&package)); return 0;
}
"#).unwrap();
    let include = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("include");
    let object = stage.path().join("models.o");
    let output = Command::new("cc")
        .args(["-std=c17", "-I"])
        .arg(&include)
        .arg("-I")
        .arg(stage.path().join("include"))
        .arg("-c")
        .arg(stage.path().join("src/models.c"))
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (compiler, language, standard) in [
        ("gcc", "c", "c17"),
        ("clang", "c", "c17"),
        ("g++", "c++", "c++17"),
        ("clang++", "c++", "c++17"),
    ] {
        let executable = stage.path().join(compiler);
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
            ])
            .arg(&include)
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(&main)
            .args(["-x", "none"])
            .arg(&object)
            .arg(&library)
            .arg(format!(
                "-Wl,-rpath,{}",
                library.parent().unwrap().display()
            ))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for (mode, expected) in expected
            .iter()
            .map(String::as_str)
            .chain(std::iter::once("c_plan_builder_id_unknown"))
            .chain(std::iter::once(expected[0].as_str()))
            .chain(std::iter::once(create_expected.as_str()))
            .chain(std::iter::repeat_n(expected[0].as_str(), 2))
            .enumerate()
        {
            let output = Command::new(&executable)
                .arg(mode.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{compiler},mode={mode},exit={:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                expected,
                "{compiler},mode={mode}"
            );
        }
    }
}

#[test]
fn bound_raw_build_accepts_matching_fingerprints_and_refuses_foreign_packages_without_poisoning() {
    let fixture = purchase_package();
    let package = OpenPackage::new(&fixture);
    let equivalent = OpenPackage::new(&fixture);
    assert_ne!(package.handle, equivalent.handle);
    let foreign_source = purchase_package::SOURCE
        .replace("attributes:\n", "attributes:\n  extra: {value: string}\n");
    let foreign_fixture = purchase_package::package_from_source(&foreign_source, PREFIX);
    let foreign = OpenPackage::new(&foreign_fixture);
    let token = item_model_token(&fixture);
    let mut pointer = ptr::null_mut();
    // SAFETY: live package and disjoint owner output; all later inputs/outputs are similarly borrowed for the call.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_new_with_package(package.handle, ptr::null(), &mut pointer),
            TypeBridgeStatus::Ok
        );
    }
    let builder = B(pointer);
    // SAFETY: the recording, token and scalar outputs remain live and disjoint.
    unsafe {
        let model = builder.id(|b, out| {
            type_bridge_plan_builder_type_token(b, &token, builder.text("shop_plan_open"), out)
        });
        builder.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        builder.ok(|b| type_bridge_plan_builder_count(b));
    }
    let expected = builder.build(&package).unwrap();
    assert_eq!(builder.build(&equivalent).unwrap(), expected);
    assert_eq!(
        builder.build(&foreign).unwrap_err().1,
        "generated_token_package_mismatch"
    );
    assert_eq!(builder.build(&package).unwrap(), expected);
    let mut captured = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: the authority accessor borrows live immutable package bytes.
    unsafe {
        assert_eq!(
            type_bridge_schema_package_authority_json(package.handle, &mut captured),
            TypeBridgeStatus::Ok
        );
    }
    assert!(captured.length >= size_of::<*mut Plan>());
    // SAFETY: the complete view is borrowed from the live original package.
    let before =
        unsafe { std::slice::from_raw_parts(captured.data, size_of::<*mut Plan>()) }.to_vec();
    let mut diagnostics = ptr::null_mut();
    // SAFETY: an output alias into the captured original package must be refused before writing,
    // even when the explicit argument is a separate same-fingerprint package handle.
    unsafe {
        assert_eq!(
            type_bridge_plan_builder_build(
                builder.0,
                equivalent.handle,
                captured.data.cast_mut().cast(),
                &mut diagnostics
            ),
            TypeBridgeStatus::InvalidArgument
        );
        assert!(diagnostics.is_null());
        assert_eq!(
            std::slice::from_raw_parts(captured.data, before.len()),
            before
        );
    }
    assert_eq!(builder.build(&equivalent).unwrap(), expected);
    let raw = B::new();
    // SAFETY: the raw recording is unbound and preserves explicit-package validation.
    unsafe {
        let model =
            raw.id(|b, out| type_bridge_plan_builder_type(b, ENTITY, raw.text("item"), out));
        raw.id(|b, out| type_bridge_plan_builder_exact(b, model, out));
        raw.ok(|b| type_bridge_plan_builder_count(b));
    }
    assert!(raw.build(&foreign).is_ok());
}
