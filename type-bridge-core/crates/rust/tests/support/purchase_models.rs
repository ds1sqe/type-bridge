//! The one-query purchase schema as a generated-style package: hand-written
//! models implementing the generated SPI, and the package compiled from
//! Split-YAML at run time. Shared by the SDK plan tests and the server's
//! live staged tests.
#![allow(dead_code)]

use type_bridge::__codegen::{
    EntityModel, FieldToken, Model, NominalUpcast, RelationModel, RolePlayerBinding, RoleToken,
    RoleTokenCompatible, ThingKind, ThingModel,
};
use type_bridge::Exact;
use type_bridge::schema::{Schema, SchemaPackage, sealed};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, encode_declared_schema};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_schema::{
    ManagedDeltaContext, SchemaDocumentSet, build_schema_authority, encode_schema_authority,
    normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::RustEmitter;

#[path = "purchase_plan.rs"]
mod purchase_plan;
pub use purchase_plan::{PROFILE, SCOPE, SOURCE};

#[derive(Clone, Copy, Debug)]
pub struct Shop;
impl sealed::Sealed for Shop {}
impl Schema for Shop {}

macro_rules! thing {
    ($name:ident, $kind:ident, $marker:ident, $json:literal) => {
        #[derive(Debug)]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl Model for $name {
            type Schema = Shop;
            const TYPE_ID_JSON: &'static str = $json;
        }
        impl ThingModel for $name {
            fn thing_kind() -> ThingKind {
                ThingKind::$kind
            }
        }
        impl $marker for $name {}
        impl NominalUpcast<$name> for $name {}
    };
}
thing!(
    Item,
    Entity,
    EntityModel,
    r#"{"kind":"entity","label":"item"}"#
);
thing!(
    Account,
    Entity,
    EntityModel,
    r#"{"kind":"entity","label":"account"}"#
);
thing!(
    Purchase,
    Relation,
    RelationModel,
    r#"{"kind":"relation","label":"purchase"}"#
);

/// The accepted players of `purchase:buyer`.
pub struct BuyerPlayers;
impl RolePlayerBinding<Account, Exact> for BuyerPlayers {}
/// The accepted players of `purchase:item`.
pub struct ItemPlayers;
impl RolePlayerBinding<Item, Exact> for ItemPlayers {}
impl RoleTokenCompatible<Purchase, BuyerPlayers> for Purchase {}
impl RoleTokenCompatible<Purchase, ItemPlayers> for Purchase {}

pub struct ItemType;
#[allow(non_upper_case_globals)]
impl ItemType {
    pub const item_id: FieldToken<Item, String> = FieldToken::new(
        r#"{"attribute":"item-id","owner":{"kind":"entity","label":"item"}}"#,
        "{}",
    );
    pub const stock: FieldToken<Item, i64> = FieldToken::new(
        r#"{"attribute":"stock","owner":{"kind":"entity","label":"item"}}"#,
        "{}",
    );
    pub const price: FieldToken<Item, i64> = FieldToken::new(
        r#"{"attribute":"price","owner":{"kind":"entity","label":"item"}}"#,
        "{}",
    );
}
pub struct AccountType;
#[allow(non_upper_case_globals)]
impl AccountType {
    pub const account_id: FieldToken<Account, String> = FieldToken::new(
        r#"{"attribute":"account-id","owner":{"kind":"entity","label":"account"}}"#,
        "{}",
    );
    pub const balance: FieldToken<Account, i64> = FieldToken::new(
        r#"{"attribute":"balance","owner":{"kind":"entity","label":"account"}}"#,
        "{}",
    );
}
pub struct PurchaseType;
#[allow(non_upper_case_globals)]
impl PurchaseType {
    pub const order_id: FieldToken<Purchase, String> = FieldToken::new(
        r#"{"attribute":"order-id","owner":{"kind":"relation","label":"purchase"}}"#,
        "{}",
    );
    pub const buyer: RoleToken<Purchase, BuyerPlayers> =
        RoleToken::new(r#"{"declaring_relation":"purchase","label":"buyer"}"#, "{}");
    pub const item: RoleToken<Purchase, ItemPlayers> =
        RoleToken::new(r#"{"declaring_relation":"purchase","label":"item"}"#, "{}");
}

/// The purchase as the builder writes it, as in the Rust guide.
pub fn purchase_plan(
    package: &SchemaPackage<Shop>,
    order: &str,
    item_id: &str,
    account_id: &str,
) -> type_bridge::Result<type_bridge::plan::Plan<Shop>> {
    use type_bridge::plan::Plan;
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let buyer = plan.exact::<Account>();
    let order_input = plan.input::<String>("order_id");
    let item_input = plan.input::<String>("item_id");
    let account_input = plan.input::<String>("account_id");
    let item_key = plan.field(item, ItemType::item_id);
    let buyer_key = plan.field(buyer, AccountType::account_id);
    let stock = plan.field(item, ItemType::stock);
    let price = plan.field(item, ItemType::price);
    let balance = plan.field(buyer, AccountType::balance);
    plan.where_(item_key.eq(item_input))
        .where_(buyer_key.eq(account_input))
        .where_(stock.gt(0_i64))
        .where_(balance.ge(price));
    let next_stock = plan.let_("next_stock", stock - 1_i64);
    let next_balance = plan.let_("next_balance", balance - price);
    plan.update(item, ItemType::stock, next_stock).update(
        buyer,
        AccountType::balance,
        next_balance,
    );
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, order_input)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    plan.row([
        order_input.value(order.to_owned()),
        item_input.value(item_id.to_owned()),
        account_input.value(account_id.to_owned()),
    ]);
    plan.build(package)
}

fn leak(bytes: Vec<u8>) -> &'static str {
    Box::leak(String::from_utf8(bytes).unwrap().into_boxed_str())
}

/// The declared schema and managed semantics of [`package`].
pub fn authority() -> (DeclaredSchema, ManagedSemanticSchemaFingerprint) {
    let (declared, authority, _) = compile();
    (
        declared,
        authority.managed_state().managed_semantic_schema().clone(),
    )
}

fn compile() -> (
    DeclaredSchema,
    type_bridge_schema::VerifiedSchemaAuthority,
    type_bridge_schema::ResolvedSchema,
) {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("purchase.yaml").unwrap(), SOURCE)]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new(PROFILE).unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let authority = build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();
    (declared, authority, resolved)
}

/// The generated package for [`SOURCE`].
pub fn package() -> SchemaPackage<Shop> {
    let (declared, authority, resolved) = compile();
    let emitter = RustEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    SchemaPackage::new_with_authority(
        leak(to_canonical_json(projection.semantic_fingerprint()).unwrap()),
        leak(to_canonical_json(projection.projection_fingerprint()).unwrap()),
        leak(to_canonical_json(&projection).unwrap()),
        leak(encode_schema_authority(&authority)),
        leak(encode_declared_schema(&declared).unwrap()),
        SCOPE,
        PROFILE,
    )
}

/// The hand-built purchase under [`authority`].
pub fn hand_built_purchase(
    order: &str,
    item_id: &str,
    account_id: &str,
) -> type_bridge_contract::query_plan::v3::QueryPlanV3 {
    let (declared, semantics) = authority();
    purchase_plan::hand_built_purchase(&declared, semantics, order, item_id, account_id)
}
