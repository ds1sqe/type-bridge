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
    TypeBridgeByteView, TypeBridgeExecutionDiagnosticViewV1, TypeBridgeSchemaPackageDescriptorV1,
    TypeBridgeStatus,
};

#[path = "support/purchase_package.rs"]
mod purchase_package;
#[cfg(not(windows))]
#[path = "support/temp.rs"]
mod temp;

use purchase_package::{PREFIX, PurchasePackage, purchase_package};
#[cfg(not(windows))]
use temp::TempDirectory;

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

unsafe extern "C" {
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
