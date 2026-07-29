//! Binding-target configuration and reproducible projection fingerprints.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Serialize, Serializer};

use crate::codec::{FormatVersion, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::{AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind};
use crate::limits::MAX_CANONICAL_COLLECTION_LEN;
use crate::schema::{
    AnnotationFact, AnnotationFactId, AnnotationSubjectId, OwnsFactId, PlaysFactId,
    SchemaAnnotationValue, SchemaFactId, SubFactId, ValueFactId,
};
use crate::schema_fingerprint::SemanticSchemaFingerprint;
use crate::value::{Cardinality, ValueTypeTag};

const MAX_PROJECTION_COMPONENT_ID_BYTES: usize = 255;
const PYTHON_GENERATOR_HANDLER_ID: &str = "typebridge.generator.python";
const TYPESCRIPT_GENERATOR_HANDLER_ID: &str = "typebridge.generator.typescript";
const RUST_GENERATOR_HANDLER_ID: &str = "typebridge.generator.rust";
const CODE_RESOURCE_DOMAIN: &str = "typebridge.binding.code-resource";
const RAW_BYTES_CANONICALIZATION: &str = "typebridge.raw-bytes/v1";
const BINDING_PROJECTION_DOMAIN: &str = "typebridge.binding.projection";
const BINDING_PROJECTION_CANONICALIZATION: &str = "typebridge.binding-projection/v1";
const BINDING_PROJECTION_CONTENT_DOMAIN: &str = "typebridge.binding.projection-content";
const MAX_TARGET_IDENTIFIER_BYTES: usize = 255;

/// A binding target with a consumed Phase 3 projection contract.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingTarget {
    /// Generated Python source and typing artifacts.
    Python,
    /// Generated TypeScript source and declarations.
    #[serde(rename = "typescript")]
    TypeScript,
    /// Generated native Rust types and schema tokens.
    Rust,
}

impl BindingTarget {
    const fn required_generator_handler_id(self) -> &'static str {
        match self {
            Self::Python => PYTHON_GENERATOR_HANDLER_ID,
            Self::TypeScript => TYPESCRIPT_GENERATOR_HANDLER_ID,
            Self::Rust => RUST_GENERATOR_HANDLER_ID,
        }
    }
}

/// The exact label-to-Python-name transformation consumed by the emitter.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum PythonNamingPolicy {
    /// The first collision-checked TypeBridge Python naming policy.
    #[serde(rename = "typebridge.python/v1")]
    TypeBridgeV1,
}

/// The exact label-to-TypeScript-name transformation consumed by the emitter.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TypeScriptNamingPolicy {
    /// The first collision-checked TypeBridge TypeScript naming policy.
    #[serde(rename = "typebridge.typescript/v1")]
    TypeBridgeV1,
}

/// The exact label-to-Rust-name transformation consumed by native projection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum RustNamingPolicy {
    /// The first collision-checked TypeBridge Rust naming policy.
    #[serde(rename = "typebridge.rust/v1")]
    TypeBridgeV1,
}

/// The generated Rust construction surface committed by the projection fingerprint.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum RustCreatePolicy {
    /// Emit a public `{Model}Create` input with private fields, checked `try_new`,
    /// and manager insertion that consumes the validated input. Abstract or
    /// otherwise nonconstructible models expose no create input.
    #[serde(rename = "typebridge.rust.validated-create-input/v1")]
    ValidatedInputV1,
}

/// Target-specific options that are consumed by a shipped emitter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "binding")]
pub enum ProjectionConfig {
    /// Python projection options.
    #[serde(rename = "python")]
    Python {
        /// Versioned Python naming behavior used for every generated symbol.
        naming_policy: PythonNamingPolicy,
    },
    /// TypeScript projection options.
    #[serde(rename = "typescript")]
    TypeScript {
        /// Versioned TypeScript naming behavior used for every generated symbol.
        naming_policy: TypeScriptNamingPolicy,
    },
    /// Native Rust projection options.
    #[serde(rename = "rust")]
    Rust {
        /// Versioned Rust naming behavior used for every generated symbol.
        naming_policy: RustNamingPolicy,
        /// Versioned checked construction surface generated for constructible models.
        create_policy: RustCreatePolicy,
    },
}

impl ProjectionConfig {
    /// Construct the initial Python projection configuration.
    #[must_use]
    pub const fn python() -> Self {
        Self::Python {
            naming_policy: PythonNamingPolicy::TypeBridgeV1,
        }
    }

    /// Construct the initial TypeScript projection configuration.
    #[must_use]
    pub const fn typescript() -> Self {
        Self::TypeScript {
            naming_policy: TypeScriptNamingPolicy::TypeBridgeV1,
        }
    }

    /// Construct the initial native Rust projection configuration.
    #[must_use]
    pub const fn rust() -> Self {
        Self::Rust {
            naming_policy: RustNamingPolicy::TypeBridgeV1,
            create_policy: RustCreatePolicy::ValidatedInputV1,
        }
    }

    /// Return the binding target that consumes this configuration.
    #[must_use]
    pub const fn target(&self) -> BindingTarget {
        match self {
            Self::Python { .. } => BindingTarget::Python,
            Self::TypeScript { .. } => BindingTarget::TypeScript,
            Self::Rust { .. } => BindingTarget::Rust,
        }
    }

    /// Return the TypeScript naming policy when this is a TypeScript config.
    #[must_use]
    pub const fn typescript_naming_policy(&self) -> Option<TypeScriptNamingPolicy> {
        match self {
            Self::TypeScript { naming_policy } => Some(*naming_policy),
            Self::Python { .. } | Self::Rust { .. } => None,
        }
    }

    /// Return the Rust naming policy when this is a Rust config.
    #[must_use]
    pub const fn rust_naming_policy(&self) -> Option<RustNamingPolicy> {
        match self {
            Self::Rust { naming_policy, .. } => Some(*naming_policy),
            Self::Python { .. } | Self::TypeScript { .. } => None,
        }
    }

    /// Return the checked Rust create policy when this is a Rust config.
    #[must_use]
    pub const fn rust_create_policy(&self) -> Option<RustCreatePolicy> {
        match self {
            Self::Rust { create_policy, .. } => Some(*create_policy),
            Self::Python { .. } | Self::TypeScript { .. } => None,
        }
    }
}

fn validate_component_id(value: String) -> Result<String, Diagnostic> {
    let segments = value.split('.').collect::<Vec<_>>();
    let valid_segment = |segment: &str| {
        let mut bytes = segment.bytes();
        bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
    };
    if value.len() <= MAX_PROJECTION_COMPONENT_ID_BYTES
        && segments.len() >= 2
        && segments.iter().all(|segment| valid_segment(segment))
    {
        Ok(value)
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "malformed_projection_component_id",
            "projection component ID must be a bounded lowercase namespaced identifier",
        ))
    }
}

macro_rules! projection_component_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Validate and construct a namespaced component identity.
            pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
                Ok(Self(validate_component_id(value.into())?))
            }

            /// Return the canonical identity spelling.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }
    };
}

projection_component_id!(
    ProjectionHandlerId,
    "A stable identity for one generator or projection handler."
);
projection_component_id!(
    CodeResourceId,
    "A stable identity for exact code-resource bytes referenced during emission."
);

/// A nonzero behavior version for one projection handler.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectionHandlerVersion(u16);

impl ProjectionHandlerVersion {
    /// The initial handler behavior version.
    pub const V1: Self = Self(1);

    /// Validate and construct a handler behavior version.
    pub fn new(value: u16) -> Result<Self, Diagnostic> {
        if value == 0 {
            Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "invalid_projection_handler_version",
                "projection handler version must be nonzero",
            ))
        } else {
            Ok(Self(value))
        }
    }

    /// Return the numeric behavior version.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// The identity and behavior version of a handler that participated in emission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectionHandler {
    id: ProjectionHandlerId,
    version: ProjectionHandlerVersion,
}

impl ProjectionHandler {
    /// Construct one executed projection-handler identity.
    pub fn new(id: impl Into<String>, version: u16) -> Result<Self, Diagnostic> {
        Ok(Self {
            id: ProjectionHandlerId::new(id)?,
            version: ProjectionHandlerVersion::new(version)?,
        })
    }

    /// Construct the initial built-in Python generator identity.
    #[must_use]
    pub fn python_v1() -> Self {
        Self {
            id: ProjectionHandlerId::new(PYTHON_GENERATOR_HANDLER_ID)
                .expect("the built-in Python generator ID is valid"),
            version: ProjectionHandlerVersion::V1,
        }
    }

    /// Construct the initial built-in TypeScript generator identity.
    #[must_use]
    pub fn typescript_v1() -> Self {
        Self {
            id: ProjectionHandlerId::new(TYPESCRIPT_GENERATOR_HANDLER_ID)
                .expect("built-in TypeScript projection handler ID is valid"),
            version: ProjectionHandlerVersion::V1,
        }
    }

    /// Construct the initial built-in native Rust generator identity.
    #[must_use]
    pub fn rust_v1() -> Self {
        Self {
            id: ProjectionHandlerId::new(RUST_GENERATOR_HANDLER_ID)
                .expect("built-in Rust projection handler ID is valid"),
            version: ProjectionHandlerVersion::V1,
        }
    }

    /// Return the handler identity.
    #[must_use]
    pub const fn id(&self) -> &ProjectionHandlerId {
        &self.id
    }

    /// Return the handler behavior version.
    #[must_use]
    pub const fn version(&self) -> ProjectionHandlerVersion {
        self.version
    }
}

/// A domain-separated digest of exact code-resource bytes used by an emitter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodeResourceDigest {
    id: CodeResourceId,
    content_fingerprint: Fingerprint,
}

impl CodeResourceDigest {
    /// Hash the exact bytes of one referenced code resource.
    pub fn from_bytes(id: impl Into<String>, bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self {
            id: CodeResourceId::new(id)?,
            content_fingerprint: Fingerprint::compute(
                FingerprintDomain::new(CODE_RESOURCE_DOMAIN)?,
                CanonicalizationVersion::new(RAW_BYTES_CANONICALIZATION)?,
                None,
                bytes,
            ),
        })
    }

    /// Return the resource identity.
    #[must_use]
    pub const fn id(&self) -> &CodeResourceId {
        &self.id
    }

    /// Return the domain-separated exact-content fingerprint.
    #[must_use]
    pub const fn content_fingerprint(&self) -> &Fingerprint {
        &self.content_fingerprint
    }
}

#[derive(Serialize)]
struct BindingProjectionView<'a> {
    format_version: FormatVersion,
    target: BindingTarget,
    semantic_schema_fingerprint: &'a SemanticSchemaFingerprint,
    config: &'a ProjectionConfig,
    generator_handlers: Vec<&'a ProjectionHandler>,
    referenced_code_resources: Vec<&'a CodeResourceDigest>,
}

fn ordered_handlers(
    target: BindingTarget,
    handlers: &[ProjectionHandler],
) -> Result<Vec<&ProjectionHandler>, Diagnostic> {
    let mut ordered = handlers.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| match left.id().cmp(right.id()) {
        Ordering::Equal => left.version().cmp(&right.version()),
        ordering => ordering,
    });
    if ordered.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
        return Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "duplicate_projection_handler_id",
            "a projection handler identity may appear only once",
        ));
    }
    if !ordered
        .iter()
        .any(|handler| handler.id().as_str() == target.required_generator_handler_id())
    {
        return Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "missing_target_projection_handler",
            "projection fingerprint inputs omit the target's generator handler",
        ));
    }
    Ok(ordered)
}

fn ordered_resources(
    resources: &[CodeResourceDigest],
) -> Result<Vec<&CodeResourceDigest>, Diagnostic> {
    let mut ordered = resources.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.id().cmp(right.id()));
    if ordered.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
        return Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "duplicate_projection_resource_id",
            "a referenced code-resource identity may appear only once",
        ));
    }
    Ok(ordered)
}

/// Produce the exact canonical preimage for a binding-projection fingerprint.
pub fn canonical_binding_projection_bytes(
    target: BindingTarget,
    semantic_schema: &SemanticSchemaFingerprint,
    config: &ProjectionConfig,
    handlers: &[ProjectionHandler],
    resources: &[CodeResourceDigest],
) -> Result<Vec<u8>, Diagnostic> {
    if config.target() != target {
        return Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "projection_config_target_mismatch",
            "projection configuration belongs to a different binding target",
        ));
    }
    let view = BindingProjectionView {
        format_version: FormatVersion::V1,
        target,
        semantic_schema_fingerprint: semantic_schema,
        config,
        generator_handlers: ordered_handlers(target, handlers)?,
        referenced_code_resources: ordered_resources(resources)?,
    };
    to_canonical_json(&view)
}

/// Fingerprint of one binding projection and every input that can alter it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BindingProjectionFingerprint(Fingerprint);

impl BindingProjectionFingerprint {
    /// Compute a target-specific projection fingerprint from trusted inputs.
    pub fn compute(
        target: BindingTarget,
        semantic_schema: &SemanticSchemaFingerprint,
        config: &ProjectionConfig,
        handlers: &[ProjectionHandler],
        resources: &[CodeResourceDigest],
    ) -> Result<Self, Diagnostic> {
        let canonical = canonical_binding_projection_bytes(
            target,
            semantic_schema,
            config,
            handlers,
            resources,
        )?;
        let semantic_profile = semantic_schema
            .as_fingerprint()
            .semantic_profile()
            .cloned()
            .ok_or_else(|| {
                Diagnostic::stable(
                    DiagnosticCategory::InvalidContract,
                    "projection_semantic_profile_missing",
                    "semantic schema fingerprint does not carry its semantic profile",
                )
            })?;
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(BINDING_PROJECTION_DOMAIN)?,
            CanonicalizationVersion::new(BINDING_PROJECTION_CANONICALIZATION)?,
            Some(semantic_profile),
            &canonical,
        )))
    }

    /// Return the generic fingerprint metadata and digest.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    /// Compute a fingerprint that additionally commits to canonical projection content.
    pub fn compute_with_projection(
        target: BindingTarget,
        semantic_schema: &SemanticSchemaFingerprint,
        config: &ProjectionConfig,
        handlers: &[ProjectionHandler],
        resources: &[CodeResourceDigest],
        canonical_projection: &[u8],
    ) -> Result<Self, Diagnostic> {
        #[derive(Serialize)]
        struct CompleteProjectionView<'a> {
            inputs: BindingProjectionView<'a>,
            projection_content: Fingerprint,
        }

        if config.target() != target {
            return Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "projection_config_target_mismatch",
                "projection configuration belongs to a different binding target",
            ));
        }
        let inputs = BindingProjectionView {
            format_version: FormatVersion::V1,
            target,
            semantic_schema_fingerprint: semantic_schema,
            config,
            generator_handlers: ordered_handlers(target, handlers)?,
            referenced_code_resources: ordered_resources(resources)?,
        };
        let projection_content = Fingerprint::compute(
            FingerprintDomain::new(BINDING_PROJECTION_CONTENT_DOMAIN)?,
            CanonicalizationVersion::new(BINDING_PROJECTION_CANONICALIZATION)?,
            semantic_schema.as_fingerprint().semantic_profile().cloned(),
            canonical_projection,
        );
        let canonical = to_canonical_json(&CompleteProjectionView {
            inputs,
            projection_content,
        })?;
        let semantic_profile = semantic_schema
            .as_fingerprint()
            .semantic_profile()
            .cloned()
            .ok_or_else(|| {
                Diagnostic::stable(
                    DiagnosticCategory::InvalidContract,
                    "projection_semantic_profile_missing",
                    "semantic schema fingerprint does not carry its semantic profile",
                )
            })?;
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(BINDING_PROJECTION_DOMAIN)?,
            CanonicalizationVersion::new(BINDING_PROJECTION_CANONICALIZATION)?,
            Some(semantic_profile),
            &canonical,
        )))
    }
}

fn serialize_map_values<S, K, V>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
{
    map.values().collect::<Vec<_>>().serialize(serializer)
}

#[derive(Serialize)]
struct RoleUpcastEntry<'a> {
    role: &'a RoleId,
    ancestors: &'a [RoleId],
}

fn serialize_role_upcasts<S>(
    map: &BTreeMap<RoleId, Vec<RoleId>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    map.iter()
        .map(|(role, ancestors)| RoleUpcastEntry { role, ancestors })
        .collect::<Vec<_>>()
        .serialize(serializer)
}

fn invalid_projection(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::InvalidContract, code, message)
}

fn ensure_collection_limit(length: usize, code: &'static str) -> Result<(), Diagnostic> {
    if length > MAX_CANONICAL_COLLECTION_LEN {
        Err(Diagnostic::stable(
            DiagnosticCategory::ResourceLimit,
            code,
            "projection collection exceeds the canonical collection limit",
        ))
    } else {
        Ok(())
    }
}

/// A validated target-language identifier emitted verbatim by a generator.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TargetIdentifier(String);

impl TargetIdentifier {
    /// Validate one ASCII Python identifier under the frozen Python-v1 policy.
    pub fn python(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = value.len() <= MAX_TARGET_IDENTIFIER_BYTES
            && bytes
                .next()
                .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric());
        if !valid || is_python_keyword(&value) {
            return Err(invalid_projection(
                "invalid_python_projection_identifier",
                "projected Python name is not a bounded non-keyword identifier",
            ));
        }
        Ok(Self(value))
    }

    /// Validate one ASCII TypeScript identifier under the frozen TypeScript-v1 policy.
    pub fn typescript(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = value.len() <= MAX_TARGET_IDENTIFIER_BYTES
            && bytes
                .next()
                .is_some_and(|byte| byte == b'_' || byte == b'$' || byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte == b'$' || byte.is_ascii_alphanumeric());
        if !valid || is_typescript_keyword(&value) {
            return Err(invalid_projection(
                "invalid_typescript_projection_identifier",
                "projected TypeScript name is not a bounded non-keyword identifier",
            ));
        }
        Ok(Self(value))
    }

    /// Validate one ASCII Rust identifier under the frozen Rust-v1 policy.
    pub fn rust(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = value != "_"
            && value.len() <= MAX_TARGET_IDENTIFIER_BYTES
            && bytes
                .next()
                .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric());
        if !valid || is_rust_keyword(&value) {
            return Err(invalid_projection(
                "invalid_rust_projection_identifier",
                "projected Rust name is not a bounded non-keyword ASCII identifier",
            ));
        }
        Ok(Self(value))
    }

    /// Return the exact target spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_python_keyword(value: &str) -> bool {
    matches!(
        value,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "case"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "match"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

fn is_typescript_keyword(value: &str) -> bool {
    matches!(
        value,
        "abstract"
            | "any"
            | "as"
            | "asserts"
            | "async"
            | "await"
            | "bigint"
            | "boolean"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "constructor"
            | "continue"
            | "debugger"
            | "declare"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "from"
            | "function"
            | "get"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "infer"
            | "instanceof"
            | "interface"
            | "is"
            | "keyof"
            | "let"
            | "module"
            | "namespace"
            | "never"
            | "new"
            | "null"
            | "number"
            | "object"
            | "of"
            | "out"
            | "override"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "readonly"
            | "require"
            | "return"
            | "satisfies"
            | "set"
            | "static"
            | "string"
            | "super"
            | "switch"
            | "symbol"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "undefined"
            | "unique"
            | "unknown"
            | "using"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "Self"
            | "abstract"
            | "as"
            | "async"
            | "await"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    )
}

/// Whether a projected model use is complete or a nonrecursive reference.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectedModelForm {
    /// A complete materialized model.
    Complete,
    /// An identity/reference-only model.
    Reference,
}

/// One typed use of a projected model.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ProjectedModelUse {
    id: TypeId,
    form: ProjectedModelForm,
}

impl ProjectedModelUse {
    /// Construct one typed model use.
    #[must_use]
    pub const fn new(id: TypeId, form: ProjectedModelForm) -> Self {
        Self { id, form }
    }
    /// Return the model identity.
    #[must_use]
    pub const fn id(&self) -> &TypeId {
        &self.id
    }
    /// Return the requested materialization form.
    #[must_use]
    pub const fn form(&self) -> ProjectedModelForm {
        self.form
    }
}

/// A fully resolved type position used by projected signatures.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProjectedTypeRef {
    /// A built-in scalar domain.
    Scalar(ValueTypeTag),
    /// A model identity and materialization form.
    Model(ProjectedModelUse),
    /// A schema struct value.
    Struct(StructId),
}

/// Scalar versus collection container shape derived from cardinality.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectedContainer {
    /// At most one value.
    Scalar,
    /// Multiple values in a generated sequence container.
    Sequence,
}

/// Requiredness and container form derived from one resolved cardinality.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectedMultiplicity {
    cardinality: Cardinality,
    required: bool,
    container: ProjectedContainer,
}

impl ProjectedMultiplicity {
    /// Derive an honest input/read shape from resolved cardinality.
    #[must_use]
    pub const fn from_cardinality(cardinality: Cardinality) -> Self {
        let container = match cardinality.max() {
            Some(0 | 1) => ProjectedContainer::Scalar,
            Some(_) | None => ProjectedContainer::Sequence,
        };
        Self {
            cardinality,
            required: cardinality.min() > 0,
            container,
        }
    }
    /// Return the exact resolved cardinality.
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }
    /// Report whether the generated field is required.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.required
    }
    /// Return the generated container category.
    #[must_use]
    pub const fn container(&self) -> ProjectedContainer {
        self.container
    }
}

/// One effective annotation retained in runtime projection metadata.
///
/// The ID names the effective projected subject, not the direct declaration
/// from which an inherited value originated. Type, ownership, related-role,
/// and playing annotations therefore use the actual projected owner or player.
/// For an inherited related role, the annotation-only subject remints the role
/// label under the effective relation while [`RoleTokenProjection::role`]
/// retains the canonical declaring-role identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectedAnnotation {
    id: AnnotationFactId,
    value: SchemaAnnotationValue,
}

impl ProjectedAnnotation {
    /// Construct one projected annotation through the authoritative annotation contract.
    pub fn new(id: AnnotationFactId, value: SchemaAnnotationValue) -> Result<Self, Diagnostic> {
        AnnotationFact::new(id.clone(), value.clone())?;
        Ok(Self { id, value })
    }
    /// Return the effective-subject annotation identity.
    #[must_use]
    pub const fn id(&self) -> &AnnotationFactId {
        &self.id
    }
    /// Return the effective annotation value.
    #[must_use]
    pub const fn value(&self) -> &SchemaAnnotationValue {
        &self.value
    }
}

/// One owner-branded owned-attribute query token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FieldTokenProjection {
    id: OwnsFactId,
    declaring_id: OwnsFactId,
    target_name: TargetIdentifier,
    multiplicity: ProjectedMultiplicity,
    key: bool,
    unique: bool,
    #[serde(serialize_with = "serialize_map_values")]
    annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
}

impl FieldTokenProjection {
    /// Construct a projected owned-attribute token.
    pub fn new(
        id: OwnsFactId,
        declaring_id: OwnsFactId,
        target_name: TargetIdentifier,
        multiplicity: ProjectedMultiplicity,
        key: bool,
        unique: bool,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    ) -> Result<Self, Diagnostic> {
        if id.attribute() != declaring_id.attribute() {
            return Err(invalid_projection(
                "invalid_projection_reference",
                "effective owns fact attribute does not match declaring owns fact attribute",
            ));
        }
        if annotations.iter().any(|(key, value)| {
            key != value.id()
                || !matches!(
                    value.id().subject(),
                    AnnotationSubjectId::Owns(subject) if subject == &id
                )
        }) {
            return Err(invalid_projection(
                "invalid_projected_owns_annotation",
                "owns annotations require matching exact effective owns subjects",
            ));
        }
        ensure_collection_limit(annotations.len(), "too_many_projected_annotations")?;
        Ok(Self {
            id,
            declaring_id,
            target_name,
            multiplicity,
            key,
            unique,
            annotations,
        })
    }
    /// Return the effective ownership identity.
    #[must_use]
    pub const fn id(&self) -> &OwnsFactId {
        &self.id
    }
    /// Return the declaring ownership identity.
    #[must_use]
    pub const fn declaring_id(&self) -> &OwnsFactId {
        &self.declaring_id
    }
    /// Return the emitted member name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return resolved requiredness and container shape.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
    /// Report key semantics.
    #[must_use]
    pub const fn is_key(&self) -> bool {
        self.key
    }
    /// Report independent uniqueness semantics.
    #[must_use]
    pub const fn is_unique(&self) -> bool {
        self.unique
    }
    /// Return effective annotations.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.annotations
    }
}

/// One owner-branded related-role query token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RoleTokenProjection {
    owner: TypeId,
    role: RoleId,
    target_name: TargetIdentifier,
    #[serde(skip_serializing_if = "Option::is_none")]
    player_union_target_name: Option<TargetIdentifier>,
    accepted_players: BTreeSet<TypeId>,
    specializes: Option<RoleId>,
    multiplicity: ProjectedMultiplicity,
    is_abstract: bool,
    #[serde(serialize_with = "serialize_map_values")]
    annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
}

impl RoleTokenProjection {
    /// Construct an effective role token for one actual relation owner.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: TypeId,
        role: RoleId,
        target_name: TargetIdentifier,
        accepted_players: BTreeSet<TypeId>,
        specializes: Option<RoleId>,
        multiplicity: ProjectedMultiplicity,
        is_abstract: bool,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    ) -> Result<Self, Diagnostic> {
        if owner.kind() != TypeKind::Relation
            || accepted_players
                .iter()
                .any(|id| !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation))
        {
            return Err(invalid_projection(
                "invalid_projected_role_token",
                "role tokens require a relation owner and entity/relation players",
            ));
        }
        ensure_collection_limit(accepted_players.len(), "too_many_projected_role_players")?;
        ensure_collection_limit(annotations.len(), "too_many_projected_annotations")?;
        Ok(Self {
            owner,
            role,
            target_name,
            player_union_target_name: None,
            accepted_players,
            specializes,
            multiplicity,
            is_abstract,
            annotations,
        })
    }
    /// Attach the explicit native player-union type name.
    #[must_use]
    pub fn with_player_union_target_name(mut self, target_name: TargetIdentifier) -> Self {
        self.player_union_target_name = Some(target_name);
        self
    }
    /// Return the actual relation model that owns the token.
    #[must_use]
    pub const fn owner(&self) -> &TypeId {
        &self.owner
    }
    /// Return the canonical declaring-role identity.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
    /// Return the emitted member name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return the explicit native player-union type name, when the target uses one.
    #[must_use]
    pub const fn player_union_target_name(&self) -> Option<&TargetIdentifier> {
        self.player_union_target_name.as_ref()
    }
    /// Return exact logical player identities.
    #[must_use]
    pub const fn accepted_players(&self) -> &BTreeSet<TypeId> {
        &self.accepted_players
    }
    /// Return the immediate specialized role, if any.
    #[must_use]
    pub const fn specializes(&self) -> Option<&RoleId> {
        self.specializes.as_ref()
    }
    /// Return resolved role cardinality shape.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
    /// Report whether the role is abstract.
    #[must_use]
    pub const fn is_abstract(&self) -> bool {
        self.is_abstract
    }
    /// Return effective relates annotations.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.annotations
    }
}

/// One direct role declaration and its immediate specialization target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeclaredRoleProjection {
    role: RoleId,
    specializes: Option<RoleId>,
}

impl DeclaredRoleProjection {
    /// Construct one direct projected role declaration.
    #[must_use]
    pub const fn new(role: RoleId, specializes: Option<RoleId>) -> Self {
        Self { role, specializes }
    }
    /// Return the declared role.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
    /// Return its immediate specialization target.
    #[must_use]
    pub const fn specializes(&self) -> Option<&RoleId> {
        self.specializes.as_ref()
    }
}

/// The exact direct subtype declaration retained by every runtime projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectSubProjection {
    id: SubFactId,
    origin: SchemaFactId,
    #[serde(serialize_with = "serialize_map_values")]
    annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
}

impl DirectSubProjection {
    /// Construct one direct subtype declaration from resolved schema values.
    pub fn new(
        id: SubFactId,
        origin: SchemaFactId,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    ) -> Result<Self, Diagnostic> {
        if origin != SchemaFactId::Sub(id.clone()) {
            return Err(invalid_projection(
                "invalid_projected_sub_origin",
                "projected subtype origin must identify its exact direct edge",
            ));
        }
        if annotations.iter().any(|(key, value)| {
            key != value.id()
                || !matches!(key.subject(), AnnotationSubjectId::Sub(subject) if subject == &id)
        }) {
            return Err(invalid_projection(
                "invalid_projected_sub_annotation",
                "subtype annotations require matching exact edge subjects",
            ));
        }
        ensure_collection_limit(annotations.len(), "too_many_projected_annotations")?;
        Ok(Self {
            id,
            origin,
            annotations,
        })
    }

    /// Return the exact direct subtype-edge identity.
    #[must_use]
    pub const fn id(&self) -> &SubFactId {
        &self.id
    }

    /// Return the direct declaration origin.
    #[must_use]
    pub const fn origin(&self) -> &SchemaFactId {
        &self.origin
    }

    /// Return annotations attached to this exact subtype edge.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.annotations
    }
}

/// The nominal declaration facet of one model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeclarationProjection {
    parent: Option<TypeId>,
    // Pre-release wire ledger: before any 2.0.0 artifact shipped,
    // binding-projection/v1 gained the exact direct subtype declaration.
    direct_sub: Option<DirectSubProjection>,
    value_type: Option<ValueTypeTag>,
    is_abstract: bool,
    is_constructible: bool,
    #[serde(serialize_with = "serialize_map_values")]
    annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    #[serde(serialize_with = "serialize_map_values")]
    value_annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    direct_fields: Vec<OwnsFactId>,
    #[serde(serialize_with = "serialize_map_values")]
    direct_roles: BTreeMap<RoleId, DeclaredRoleProjection>,
    direct_plays: BTreeSet<PlaysFactId>,
}

impl DeclarationProjection {
    /// Construct one declaration facet from direct attachment identities.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent: Option<TypeId>,
        value_type: Option<ValueTypeTag>,
        is_abstract: bool,
        is_constructible: bool,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
        direct_fields: Vec<OwnsFactId>,
        direct_roles: BTreeMap<RoleId, DeclaredRoleProjection>,
        direct_plays: BTreeSet<PlaysFactId>,
    ) -> Result<Self, Diagnostic> {
        for length in [
            annotations.len(),
            direct_fields.len(),
            direct_roles.len(),
            direct_plays.len(),
        ] {
            ensure_collection_limit(length, "projection_declaration_limit_exceeded")?;
        }
        Ok(Self {
            parent,
            direct_sub: None,
            value_type,
            is_abstract,
            is_constructible,
            annotations,
            value_annotations: BTreeMap::new(),
            direct_fields,
            direct_roles,
            direct_plays,
        })
    }
    /// Attach the direct subtype identity, origin, and annotations.
    pub fn with_direct_sub(
        mut self,
        direct_sub: Option<DirectSubProjection>,
    ) -> Result<Self, Diagnostic> {
        if direct_sub
            .as_ref()
            .is_some_and(|sub| self.parent.as_ref() != Some(sub.id().supertype()))
        {
            return Err(invalid_projection(
                "invalid_projected_sub_parent",
                "projected direct subtype edge must match the nominal parent",
            ));
        }
        self.direct_sub = direct_sub;
        Ok(self)
    }
    /// Attach effective attribute-value constraints without changing legacy construction.
    pub fn with_value_annotations(
        mut self,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    ) -> Result<Self, Diagnostic> {
        if annotations.iter().any(|(key, value)| {
            key != value.id() || !matches!(key.subject(), AnnotationSubjectId::Value(_))
        }) {
            return Err(invalid_projection(
                "invalid_projected_value_annotation",
                "attribute value annotations require matching effective value subjects",
            ));
        }
        ensure_collection_limit(annotations.len(), "too_many_projected_annotations")?;
        self.value_annotations = annotations;
        Ok(self)
    }
    /// Return the direct nominal parent.
    #[must_use]
    pub const fn parent(&self) -> Option<&TypeId> {
        self.parent.as_ref()
    }
    /// Return the exact direct subtype declaration, if this model has a parent.
    #[must_use]
    pub const fn direct_sub(&self) -> Option<&DirectSubProjection> {
        self.direct_sub.as_ref()
    }
    /// Return an attribute's effective scalar domain.
    #[must_use]
    pub const fn value_type(&self) -> Option<ValueTypeTag> {
        self.value_type
    }
    /// Report abstractness.
    #[must_use]
    pub const fn is_abstract(&self) -> bool {
        self.is_abstract
    }
    /// Report constructibility.
    #[must_use]
    pub const fn is_constructible(&self) -> bool {
        self.is_constructible
    }
    /// Return effective type annotations.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.annotations
    }
    /// Return effective attribute-value constraints.
    #[must_use]
    pub const fn value_annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.value_annotations
    }
    /// Return direct ownership attachment identities in semantic order.
    #[must_use]
    pub fn direct_fields(&self) -> &[OwnsFactId] {
        &self.direct_fields
    }
    /// Return direct role declarations.
    #[must_use]
    pub const fn direct_roles(&self) -> &BTreeMap<RoleId, DeclaredRoleProjection> {
        &self.direct_roles
    }
    /// Return direct role-playing attachments.
    #[must_use]
    pub const fn direct_plays(&self) -> &BTreeSet<PlaysFactId> {
        &self.direct_plays
    }
}

/// One owned-attribute input in a create facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CreateFieldProjection {
    token: OwnsFactId,
    value: ProjectedTypeRef,
    multiplicity: ProjectedMultiplicity,
}

impl CreateFieldProjection {
    /// Construct one create input.
    #[must_use]
    pub const fn new(
        token: OwnsFactId,
        value: ProjectedTypeRef,
        multiplicity: ProjectedMultiplicity,
    ) -> Self {
        Self {
            token,
            value,
            multiplicity,
        }
    }
    /// Return the field token identity.
    #[must_use]
    pub const fn token(&self) -> &OwnsFactId {
        &self.token
    }
    /// Return the accepted input value type.
    #[must_use]
    pub const fn value(&self) -> &ProjectedTypeRef {
        &self.value
    }
    /// Return input requiredness/container shape.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
}

/// One active related-role input in a create facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CreateRoleProjection {
    role: RoleId,
    players: BTreeSet<ProjectedModelUse>,
    multiplicity: ProjectedMultiplicity,
}

impl CreateRoleProjection {
    /// Construct one role input from its exact accepted player uses.
    pub fn new(
        role: RoleId,
        players: BTreeSet<ProjectedModelUse>,
        multiplicity: ProjectedMultiplicity,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(players.len(), "too_many_projected_role_players")?;
        Ok(Self {
            role,
            players,
            multiplicity,
        })
    }
    /// Return the canonical role identity.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
    /// Return exact accepted player forms.
    #[must_use]
    pub const fn players(&self) -> &BTreeSet<ProjectedModelUse> {
        &self.players
    }
    /// Return input requiredness/container shape.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
}

/// The exact generated constructor facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CreateProjection {
    #[serde(skip_serializing_if = "Option::is_none")]
    target_name: Option<TargetIdentifier>,
    enabled: bool,
    fields: Vec<CreateFieldProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    roles: BTreeMap<RoleId, CreateRoleProjection>,
}

impl CreateProjection {
    /// Construct one exact constructor shape.
    pub fn new(
        enabled: bool,
        fields: Vec<CreateFieldProjection>,
        roles: BTreeMap<RoleId, CreateRoleProjection>,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(fields.len(), "too_many_projected_create_fields")?;
        ensure_collection_limit(roles.len(), "too_many_projected_create_roles")?;
        Ok(Self {
            target_name: None,
            enabled,
            fields,
            roles,
        })
    }
    /// Attach the explicit generated create-input type name.
    #[must_use]
    pub fn with_target_name(mut self, target_name: TargetIdentifier) -> Self {
        self.target_name = Some(target_name);
        self
    }
    /// Return the generated create-input type name, when construction is exposed.
    #[must_use]
    pub const fn target_name(&self) -> Option<&TargetIdentifier> {
        self.target_name.as_ref()
    }
    /// Report whether generated construction is available.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }
    /// Return owned-attribute inputs in semantic order.
    #[must_use]
    pub fn fields(&self) -> &[CreateFieldProjection] {
        &self.fields
    }
    /// Return active role inputs.
    #[must_use]
    pub const fn roles(&self) -> &BTreeMap<RoleId, CreateRoleProjection> {
        &self.roles
    }
}

/// One owned-attribute field in a complete read facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadFieldProjection {
    token: OwnsFactId,
    value: ProjectedTypeRef,
    multiplicity: ProjectedMultiplicity,
}

impl ReadFieldProjection {
    /// Construct one complete-read field.
    #[must_use]
    pub const fn new(
        token: OwnsFactId,
        value: ProjectedTypeRef,
        multiplicity: ProjectedMultiplicity,
    ) -> Self {
        Self {
            token,
            value,
            multiplicity,
        }
    }
    /// Return the field token identity.
    #[must_use]
    pub const fn token(&self) -> &OwnsFactId {
        &self.token
    }
    /// Return the read value type.
    #[must_use]
    pub const fn value(&self) -> &ProjectedTypeRef {
        &self.value
    }
    /// Return read container shape.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
}

/// One active role field in a complete read facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadRoleProjection {
    role: RoleId,
    players: BTreeSet<ProjectedModelUse>,
    multiplicity: ProjectedMultiplicity,
}

impl ReadRoleProjection {
    /// Construct one complete-read role field.
    pub fn new(
        role: RoleId,
        players: BTreeSet<ProjectedModelUse>,
        multiplicity: ProjectedMultiplicity,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(players.len(), "too_many_projected_role_players")?;
        Ok(Self {
            role,
            players,
            multiplicity,
        })
    }
    /// Return the canonical role identity.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
    /// Return exact read player forms.
    #[must_use]
    pub const fn players(&self) -> &BTreeSet<ProjectedModelUse> {
        &self.players
    }
    /// Return read container shape.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
}

/// The complete materialized read facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompleteReadProjection {
    fields: Vec<ReadFieldProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    roles: BTreeMap<RoleId, ReadRoleProjection>,
    nominal_upcasts: Vec<TypeId>,
    #[serde(serialize_with = "serialize_role_upcasts")]
    role_upcasts: BTreeMap<RoleId, Vec<RoleId>>,
}

impl CompleteReadProjection {
    /// Construct one complete-read shape.
    pub fn new(
        fields: Vec<ReadFieldProjection>,
        roles: BTreeMap<RoleId, ReadRoleProjection>,
        nominal_upcasts: Vec<TypeId>,
    ) -> Result<Self, Diagnostic> {
        for length in [fields.len(), roles.len(), nominal_upcasts.len()] {
            ensure_collection_limit(length, "projection_read_limit_exceeded")?;
        }
        Ok(Self {
            fields,
            roles,
            nominal_upcasts,
            role_upcasts: BTreeMap::new(),
        })
    }
    /// Attach active-child-role to ordered ancestor-role read mappings.
    pub fn with_role_upcasts(
        mut self,
        role_upcasts: BTreeMap<RoleId, Vec<RoleId>>,
    ) -> Result<Self, Diagnostic> {
        if role_upcasts.iter().any(|(role, ancestors)| {
            !self.roles.contains_key(role)
                || ancestors.is_empty()
                || ancestors.iter().collect::<BTreeSet<_>>().len() != ancestors.len()
        }) {
            return Err(invalid_projection(
                "invalid_projected_role_upcast",
                "role upcasts require an active role and unique non-empty ancestor roles",
            ));
        }
        ensure_collection_limit(role_upcasts.len(), "projection_read_limit_exceeded")?;
        self.role_upcasts = role_upcasts;
        Ok(self)
    }
    /// Return owned-attribute fields in semantic order.
    #[must_use]
    pub fn fields(&self) -> &[ReadFieldProjection] {
        &self.fields
    }
    /// Return active role fields.
    #[must_use]
    pub const fn roles(&self) -> &BTreeMap<RoleId, ReadRoleProjection> {
        &self.roles
    }
    /// Return legal nominal upcast model identities, nearest first.
    #[must_use]
    pub fn nominal_upcasts(&self) -> &[TypeId] {
        &self.nominal_upcasts
    }
    /// Return specialized child-role to nearest-first ancestor-role mappings.
    #[must_use]
    pub const fn role_upcasts(&self) -> &BTreeMap<RoleId, Vec<RoleId>> {
        &self.role_upcasts
    }
}

/// How a binding may construct a nonrecursive reference value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceConstructionPolicy {
    /// Only an engine IID may construct a reference in the initial runtime contract.
    IidOnly,
    /// Reference construction admits typed key fallback.
    KeyFallback,
}

/// The nonrecursive identity/reference read facet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReferenceReadProjection {
    target_name: Option<TargetIdentifier>,
    key_fields: Vec<OwnsFactId>,
    construction_policy: ReferenceConstructionPolicy,
}

impl ReferenceReadProjection {
    /// Construct a reference shape; attributes deliberately have no reference class.
    pub fn new(
        target_name: Option<TargetIdentifier>,
        key_fields: Vec<OwnsFactId>,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(key_fields.len(), "too_many_projected_reference_keys")?;
        let mut unique = BTreeSet::new();
        if key_fields.iter().any(|id| !unique.insert(id)) {
            return Err(invalid_projection(
                "duplicate_projected_reference_key",
                "reference key identities must be unique",
            ));
        }
        let construction_policy = if key_fields.is_empty() {
            ReferenceConstructionPolicy::IidOnly
        } else {
            ReferenceConstructionPolicy::KeyFallback
        };
        Ok(Self {
            target_name,
            key_fields,
            construction_policy,
        })
    }
    /// Return the generated reference type name, if supported.
    #[must_use]
    pub const fn target_name(&self) -> Option<&TargetIdentifier> {
        self.target_name.as_ref()
    }
    /// Return effective key fields in semantic order.
    #[must_use]
    pub fn key_fields(&self) -> &[OwnsFactId] {
        &self.key_fields
    }
    /// Return the explicit checked reference-construction policy.
    #[must_use]
    pub const fn construction_policy(&self) -> ReferenceConstructionPolicy {
        self.construction_policy
    }
}

/// Schema-only query tokens for one projected model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueryTokenProjection {
    type_id: TypeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_name: Option<TargetIdentifier>,
    #[serde(serialize_with = "serialize_map_values")]
    fields: BTreeMap<OwnsFactId, FieldTokenProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    roles: BTreeMap<RoleId, RoleTokenProjection>,
}

impl QueryTokenProjection {
    /// Construct schema-only tokens without query-plan or invocation state.
    pub fn new(
        type_id: TypeId,
        fields: BTreeMap<OwnsFactId, FieldTokenProjection>,
        roles: BTreeMap<RoleId, RoleTokenProjection>,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(fields.len(), "too_many_projected_field_tokens")?;
        ensure_collection_limit(roles.len(), "too_many_projected_role_tokens")?;
        Ok(Self {
            type_id,
            target_name: None,
            fields,
            roles,
        })
    }
    /// Attach the explicit nominal type/query-token name.
    #[must_use]
    pub fn with_target_name(mut self, target_name: TargetIdentifier) -> Self {
        self.target_name = Some(target_name);
        self
    }
    /// Return the model type token.
    #[must_use]
    pub const fn type_id(&self) -> &TypeId {
        &self.type_id
    }
    /// Return the explicit nominal type/query-token name, when the target uses one.
    #[must_use]
    pub const fn target_name(&self) -> Option<&TargetIdentifier> {
        self.target_name.as_ref()
    }
    /// Return owner-branded owned-attribute tokens.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<OwnsFactId, FieldTokenProjection> {
        &self.fields
    }
    /// Return owner-branded role tokens.
    #[must_use]
    pub const fn roles(&self) -> &BTreeMap<RoleId, RoleTokenProjection> {
        &self.roles
    }
}

/// All five runtime facets for one resolved schema type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelProjection {
    id: TypeId,
    target_name: TargetIdentifier,
    declaration: DeclarationProjection,
    create: CreateProjection,
    complete_read: CompleteReadProjection,
    reference_read: ReferenceReadProjection,
    query_tokens: QueryTokenProjection,
}

impl ModelProjection {
    /// Construct and cross-check all five facets for one model.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: TypeId,
        target_name: TargetIdentifier,
        declaration: DeclarationProjection,
        create: CreateProjection,
        complete_read: CompleteReadProjection,
        reference_read: ReferenceReadProjection,
        query_tokens: QueryTokenProjection,
    ) -> Result<Self, Diagnostic> {
        let exact_required_scalar = |multiplicity: ProjectedMultiplicity| {
            multiplicity.required()
                && multiplicity.container() == ProjectedContainer::Scalar
                && multiplicity.cardinality().min() == 1
                && multiplicity.cardinality().max() == Some(1)
        };
        let reference_keys_valid = reference_read.key_fields().iter().all(|key| {
            let Some(token) = query_tokens.fields().get(key) else {
                return false;
            };
            if !token.is_key() || !exact_required_scalar(token.multiplicity()) {
                return false;
            }
            let mut complete = complete_read
                .fields()
                .iter()
                .filter(|field| field.token() == key);
            let Some(field) = complete.next() else {
                return false;
            };
            if complete.next().is_some() || !exact_required_scalar(field.multiplicity()) {
                return false;
            }
            matches!(
                field.value(),
                ProjectedTypeRef::Model(value)
                    if value.form() == ProjectedModelForm::Complete
                        && value.id().kind() == TypeKind::Attribute
                        && value.id().label() == key.attribute().label()
            )
        });
        if (!reference_read.key_fields().is_empty() && reference_read.target_name().is_none())
            || !reference_keys_valid
        {
            return Err(invalid_projection(
                "invalid_projected_reference_key",
                "reference keys require exact required-scalar complete/query key facets",
            ));
        }
        if query_tokens.type_id() != &id
            || match (declaration.parent(), declaration.direct_sub()) {
                (None, None) => false,
                (Some(parent), Some(sub)) => {
                    sub.id().subtype() != &id || sub.id().supertype() != parent
                }
                (None, Some(_)) | (Some(_), None) => true,
            }
            || query_tokens
                .fields()
                .values()
                .any(|field| field.id().owner() != &id)
            || query_tokens
                .roles()
                .values()
                .any(|role| role.owner() != &id)
            || create
                .fields()
                .iter()
                .any(|field| !query_tokens.fields().contains_key(field.token()))
            || complete_read
                .fields()
                .iter()
                .any(|field| !query_tokens.fields().contains_key(field.token()))
            || create
                .roles()
                .iter()
                .any(|(id, role)| id != role.role() || !query_tokens.roles().contains_key(id))
            || complete_read
                .roles()
                .iter()
                .any(|(id, role)| id != role.role() || !query_tokens.roles().contains_key(id))
        {
            return Err(invalid_projection(
                "invalid_model_projection_reference",
                "model facets contain a mismatched owner or token reference",
            ));
        }
        Ok(Self {
            id,
            target_name,
            declaration,
            create,
            complete_read,
            reference_read,
            query_tokens,
        })
    }
    /// Return the schema model identity.
    #[must_use]
    pub const fn id(&self) -> &TypeId {
        &self.id
    }
    /// Return the emitted nominal name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return the nominal declaration facet.
    #[must_use]
    pub const fn declaration(&self) -> &DeclarationProjection {
        &self.declaration
    }
    /// Return the create facet.
    #[must_use]
    pub const fn create(&self) -> &CreateProjection {
        &self.create
    }
    /// Return the complete-read facet.
    #[must_use]
    pub const fn complete_read(&self) -> &CompleteReadProjection {
        &self.complete_read
    }
    /// Return the reference-read facet.
    #[must_use]
    pub const fn reference_read(&self) -> &ReferenceReadProjection {
        &self.reference_read
    }
    /// Return schema-only query tokens.
    #[must_use]
    pub const fn query_tokens(&self) -> &QueryTokenProjection {
        &self.query_tokens
    }
}

/// One ordered struct field and its emitted identifier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StructFieldProjection {
    name: Label,
    target_name: TargetIdentifier,
    value_type: ValueTypeTag,
    optional: bool,
}

impl StructFieldProjection {
    /// Construct one struct value field.
    #[must_use]
    pub const fn new(
        name: Label,
        target_name: TargetIdentifier,
        value_type: ValueTypeTag,
        optional: bool,
    ) -> Self {
        Self {
            name,
            target_name,
            value_type,
            optional,
        }
    }
    /// Return the schema field name.
    #[must_use]
    pub const fn name(&self) -> &Label {
        &self.name
    }
    /// Return the emitted field name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return the scalar value domain.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }
    /// Report optionality.
    #[must_use]
    pub const fn optional(&self) -> bool {
        self.optional
    }
}

/// One projected schema struct value type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StructProjection {
    id: StructId,
    target_name: TargetIdentifier,
    fields: Vec<StructFieldProjection>,
}

impl StructProjection {
    /// Construct one ordered struct projection.
    pub fn new(
        id: StructId,
        target_name: TargetIdentifier,
        fields: Vec<StructFieldProjection>,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(fields.len(), "too_many_projected_struct_fields")?;
        Ok(Self {
            id,
            target_name,
            fields,
        })
    }
    /// Return the struct identity.
    #[must_use]
    pub const fn id(&self) -> &StructId {
        &self.id
    }
    /// Return the emitted value-type name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return fields in semantic declaration order.
    #[must_use]
    pub fn fields(&self) -> &[StructFieldProjection] {
        &self.fields
    }
}

/// One ordered projected function parameter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionParameterProjection {
    name: Label,
    target_name: TargetIdentifier,
    type_ref: ProjectedTypeRef,
}

impl FunctionParameterProjection {
    /// Construct one typed function parameter.
    #[must_use]
    pub const fn new(
        name: Label,
        target_name: TargetIdentifier,
        type_ref: ProjectedTypeRef,
    ) -> Self {
        Self {
            name,
            target_name,
            type_ref,
        }
    }
    /// Return the schema parameter name.
    #[must_use]
    pub const fn name(&self) -> &Label {
        &self.name
    }
    /// Return the emitted parameter name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return the exact resolved parameter type.
    #[must_use]
    pub const fn type_ref(&self) -> &ProjectedTypeRef {
        &self.type_ref
    }
}

/// One projected function return element.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionReturnElementProjection {
    type_ref: ProjectedTypeRef,
    optional: bool,
}

impl FunctionReturnElementProjection {
    /// Construct one return element.
    #[must_use]
    pub const fn new(type_ref: ProjectedTypeRef, optional: bool) -> Self {
        Self { type_ref, optional }
    }
    /// Return the exact resolved result type.
    #[must_use]
    pub const fn type_ref(&self) -> &ProjectedTypeRef {
        &self.type_ref
    }
    /// Report optionality.
    #[must_use]
    pub const fn optional(&self) -> bool {
        self.optional
    }
}

/// Projected scalar, tuple, or stream function return shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "elements", rename_all = "snake_case")]
pub enum FunctionReturnProjection {
    /// One scalar result element.
    Scalar(FunctionReturnElementProjection),
    /// Two or more ordered tuple elements.
    Tuple(Vec<FunctionReturnElementProjection>),
    /// One or more ordered stream-row elements.
    Stream(Vec<FunctionReturnElementProjection>),
}

/// A schema-only typed function token/reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionProjection {
    id: FunctionId,
    target_name: TargetIdentifier,
    parameters: Vec<FunctionParameterProjection>,
    returns: FunctionReturnProjection,
    #[serde(serialize_with = "serialize_map_values")]
    annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
}

impl FunctionProjection {
    /// Construct a function token without translating its body.
    pub fn new(
        id: FunctionId,
        target_name: TargetIdentifier,
        parameters: Vec<FunctionParameterProjection>,
        returns: FunctionReturnProjection,
    ) -> Result<Self, Diagnostic> {
        ensure_collection_limit(parameters.len(), "too_many_projected_function_parameters")?;
        Ok(Self {
            id,
            target_name,
            parameters,
            returns,
            annotations: BTreeMap::new(),
        })
    }
    /// Attach effective function documentation and metadata.
    pub fn with_annotations(
        mut self,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    ) -> Result<Self, Diagnostic> {
        if annotations.iter().any(|(key, value)| {
            key != value.id() || key.subject() != &AnnotationSubjectId::Function(self.id.clone())
        }) {
            return Err(invalid_projection(
                "invalid_projected_function_annotation",
                "function annotations require the projected function subject",
            ));
        }
        ensure_collection_limit(annotations.len(), "too_many_projected_annotations")?;
        self.annotations = annotations;
        Ok(self)
    }
    /// Return the function identity.
    #[must_use]
    pub const fn id(&self) -> &FunctionId {
        &self.id
    }
    /// Return the emitted function-token name.
    #[must_use]
    pub const fn target_name(&self) -> &TargetIdentifier {
        &self.target_name
    }
    /// Return parameters in signature order.
    #[must_use]
    pub fn parameters(&self) -> &[FunctionParameterProjection] {
        &self.parameters
    }
    /// Return the native return shape.
    #[must_use]
    pub const fn returns(&self) -> &FunctionReturnProjection {
        &self.returns
    }
    /// Return function documentation and metadata.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.annotations
    }
}

/// Effective per-player metadata keyed independently from shared role tokens.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlayingProjection {
    id: PlaysFactId,
    role: RoleId,
    target_name: Option<TargetIdentifier>,
    multiplicity: ProjectedMultiplicity,
    #[serde(serialize_with = "serialize_map_values")]
    annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
}

impl PlayingProjection {
    /// Construct metadata for one exact effective playing edge.
    pub fn new(
        id: PlaysFactId,
        role: RoleId,
        multiplicity: ProjectedMultiplicity,
        annotations: BTreeMap<AnnotationFactId, ProjectedAnnotation>,
    ) -> Result<Self, Diagnostic> {
        if id.role() != &role
            || annotations.iter().any(|(key, value)| {
                key != value.id() || key.subject() != &AnnotationSubjectId::Plays(id.clone())
            })
        {
            return Err(invalid_projection(
                "invalid_playing_projection_reference",
                "playing metadata has a mismatched role or annotation subject",
            ));
        }
        ensure_collection_limit(annotations.len(), "too_many_projected_annotations")?;
        Ok(Self {
            id,
            role,
            target_name: None,
            multiplicity,
            annotations,
        })
    }
    /// Attach the owner-branded emitted plays-token name.
    #[must_use]
    pub fn with_target_name(mut self, target_name: TargetIdentifier) -> Self {
        self.target_name = Some(target_name);
        self
    }
    /// Return the exact playing identity.
    #[must_use]
    pub const fn id(&self) -> &PlaysFactId {
        &self.id
    }
    /// Return the shared canonical role identity.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
    /// Return the emitted owner-branded plays-token name when projected.
    #[must_use]
    pub const fn target_name(&self) -> Option<&TargetIdentifier> {
        self.target_name.as_ref()
    }
    /// Return the player-edge cardinality metadata.
    #[must_use]
    pub const fn multiplicity(&self) -> ProjectedMultiplicity {
        self.multiplicity
    }
    /// Return effective per-edge annotations.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationFactId, ProjectedAnnotation> {
        &self.annotations
    }
}

/// Deterministic shells-first and SCC-link emission schedule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EmissionPlan {
    model_shells: Vec<TypeId>,
    model_link_components: Vec<BTreeSet<TypeId>>,
    structs: Vec<StructId>,
    functions: Vec<FunctionId>,
}

impl EmissionPlan {
    /// Construct a deterministic two-phase emission schedule.
    pub fn new(
        model_shells: Vec<TypeId>,
        model_link_components: Vec<BTreeSet<TypeId>>,
        structs: Vec<StructId>,
        functions: Vec<FunctionId>,
    ) -> Result<Self, Diagnostic> {
        for length in [
            model_shells.len(),
            model_link_components.len(),
            structs.len(),
            functions.len(),
        ] {
            ensure_collection_limit(length, "projection_emission_limit_exceeded")?;
        }
        Ok(Self {
            model_shells,
            model_link_components,
            structs,
            functions,
        })
    }
    /// Return parent-first nominal model shell order.
    #[must_use]
    pub fn model_shells(&self) -> &[TypeId] {
        &self.model_shells
    }
    /// Return dependency-first SCC link components.
    #[must_use]
    pub fn model_link_components(&self) -> &[BTreeSet<TypeId>] {
        &self.model_link_components
    }
    /// Return stable struct emission order.
    #[must_use]
    pub fn structs(&self) -> &[StructId] {
        &self.structs
    }
    /// Return stable function-token emission order.
    #[must_use]
    pub fn functions(&self) -> &[FunctionId] {
        &self.functions
    }
}

/// A validated target-specific runtime projection derived from resolved semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeProjection {
    target: BindingTarget,
    config: ProjectionConfig,
    semantic_fingerprint: SemanticSchemaFingerprint,
    projection_fingerprint: BindingProjectionFingerprint,
    generator_handlers: Vec<ProjectionHandler>,
    code_resources: Vec<CodeResourceDigest>,
    #[serde(serialize_with = "serialize_map_values")]
    models: BTreeMap<TypeId, ModelProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    structs: BTreeMap<StructId, StructProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    functions: BTreeMap<FunctionId, FunctionProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    playing_facts: BTreeMap<PlaysFactId, PlayingProjection>,
    emission: EmissionPlan,
}

#[derive(Serialize)]
struct RuntimeProjectionContentView<'a> {
    #[serde(serialize_with = "serialize_map_values")]
    models: &'a BTreeMap<TypeId, ModelProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    structs: &'a BTreeMap<StructId, StructProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    functions: &'a BTreeMap<FunctionId, FunctionProjection>,
    #[serde(serialize_with = "serialize_map_values")]
    playing_facts: &'a BTreeMap<PlaysFactId, PlayingProjection>,
    emission: &'a EmissionPlan,
}

impl RuntimeProjection {
    /// Validate a complete projection graph and compute its content-bound fingerprint.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        target: BindingTarget,
        config: ProjectionConfig,
        semantic_fingerprint: SemanticSchemaFingerprint,
        handlers: &[ProjectionHandler],
        resources: &[CodeResourceDigest],
        models: BTreeMap<TypeId, ModelProjection>,
        structs: BTreeMap<StructId, StructProjection>,
        functions: BTreeMap<FunctionId, FunctionProjection>,
        playing_facts: BTreeMap<PlaysFactId, PlayingProjection>,
        emission: EmissionPlan,
    ) -> Result<Self, Diagnostic> {
        if config.target() != target
            || models.iter().any(|(key, value)| key != value.id())
            || structs.iter().any(|(key, value)| key != value.id())
            || functions.iter().any(|(key, value)| key != value.id())
            || playing_facts.iter().any(|(key, value)| key != value.id())
        {
            return Err(invalid_projection(
                "invalid_runtime_projection_map",
                "runtime projection map keys or target configuration are inconsistent",
            ));
        }
        for length in [
            models.len(),
            structs.len(),
            functions.len(),
            playing_facts.len(),
        ] {
            ensure_collection_limit(length, "runtime_projection_limit_exceeded")?;
        }
        if target == BindingTarget::Rust {
            let rust_names_complete = models.values().all(|model| {
                model.create().enabled() == model.create().target_name().is_some()
                    && model.query_tokens().target_name().is_some()
                    && model
                        .query_tokens()
                        .roles()
                        .values()
                        .all(|role| role.player_union_target_name().is_some())
                    && matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation)
                        == model.reference_read().target_name().is_some()
            }) && playing_facts
                .values()
                .all(|playing| playing.target_name().is_some());
            if !rust_names_complete {
                return Err(invalid_projection(
                    "missing_rust_projection_identifier",
                    "Rust projection omits a required create, reference, query-token, player-union, or plays identifier",
                ));
            }
        }
        let model_ids = models.keys().cloned().collect::<BTreeSet<_>>();
        if emission
            .model_shells()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != model_ids
            || emission.model_shells().len() != model_ids.len()
            || emission
                .model_link_components()
                .iter()
                .flat_map(BTreeSet::iter)
                .cloned()
                .collect::<BTreeSet<_>>()
                != model_ids
            || emission
                .model_link_components()
                .iter()
                .map(BTreeSet::len)
                .sum::<usize>()
                != model_ids.len()
            || emission.structs() != structs.keys().cloned().collect::<Vec<_>>()
            || emission.functions() != functions.keys().cloned().collect::<Vec<_>>()
        {
            return Err(invalid_projection(
                "invalid_projection_emission_plan",
                "emission plan does not cover each projected value exactly once",
            ));
        }
        let all_model_refs_valid = models.values().all(|model| {
            model.query_tokens().roles().values().all(|role| {
                role.accepted_players()
                    .iter()
                    .all(|id| models.contains_key(id))
            }) && model.create().roles().values().all(|role| {
                role.players()
                    .iter()
                    .all(|value| models.contains_key(value.id()))
            }) && model.complete_read().roles().values().all(|role| {
                role.players()
                    .iter()
                    .all(|value| models.contains_key(value.id()))
            })
        });
        if !all_model_refs_valid {
            return Err(invalid_projection(
                "invalid_projection_reference",
                "projection references a model that is not present",
            ));
        }
        let declaring_owners_valid = models.values().all(|model| {
            model.query_tokens().fields().values().all(|token| {
                let declaring_owner = token.declaring_id().owner();
                let mut curr = Some(model.id());
                let mut found = false;
                let mut visited = BTreeSet::new();
                while let Some(curr_id) = curr {
                    if !visited.insert(curr_id) {
                        return false;
                    }
                    if curr_id == declaring_owner {
                        found = true;
                        break;
                    }
                    curr = models.get(curr_id).and_then(|m| m.declaration().parent());
                }
                found
            })
        });
        if !declaring_owners_valid {
            return Err(invalid_projection(
                "invalid_projection_reference",
                "field token declaring owner is not the effective owner or a valid ancestor",
            ));
        }
        let model_use_is_valid = |value: &ProjectedModelUse| {
            models.get(value.id()).is_some_and(|model| {
                value.form() != ProjectedModelForm::Reference
                    || model.reference_read().target_name().is_some()
            })
        };
        let type_ref_is_valid = |value: &ProjectedTypeRef| match value {
            ProjectedTypeRef::Scalar(_) => true,
            ProjectedTypeRef::Model(value) => model_use_is_valid(value),
            ProjectedTypeRef::Struct(id) => structs.contains_key(id),
        };
        let role_exists = |role: &RoleId| {
            models.values().any(|model| {
                model.id().kind() == TypeKind::Relation
                    && model.id().label() == role.declaring_relation()
                    && model.query_tokens().roles().contains_key(role)
            })
        };
        let shell_positions = emission
            .model_shells()
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let closed_models = models.values().all(|model| {
            let declaration = model.declaration();
            let parent_is_valid = declaration.parent().is_none_or(|parent| {
                models.contains_key(parent) && shell_positions[parent] < shell_positions[model.id()]
            });
            let direct_sub_is_valid = match (declaration.parent(), declaration.direct_sub()) {
                (None, None) => true,
                (Some(parent), Some(sub)) => {
                    sub.id().subtype() == model.id() && sub.id().supertype() == parent
                }
                (None, Some(_)) | (Some(_), None) => false,
            };
            let direct_fields_are_valid = declaration
                .direct_fields()
                .iter()
                .all(|id| model.query_tokens().fields().contains_key(id));
            let direct_roles_are_valid = declaration.direct_roles().iter().all(|(id, role)| {
                id == role.role()
                    && model.query_tokens().roles().contains_key(id)
                    && role.specializes().is_none_or(&role_exists)
            });
            let direct_plays_are_valid = declaration
                .direct_plays()
                .iter()
                .all(|id| id.player() == model.id() && playing_facts.contains_key(id));
            let fields_are_valid = model.query_tokens().fields().values().all(|field| {
                models.keys().any(|id| {
                    id.kind() == TypeKind::Attribute && id.label() == field.id().attribute().label()
                })
            }) && model
                .create()
                .fields()
                .iter()
                .all(|field| type_ref_is_valid(field.value()))
                && model
                    .complete_read()
                    .fields()
                    .iter()
                    .all(|field| type_ref_is_valid(field.value()));
            let roles_are_valid = model
                .query_tokens()
                .roles()
                .values()
                .all(|role| role.specializes().is_none_or(&role_exists))
                && model
                    .create()
                    .roles()
                    .values()
                    .all(|role| role.players().iter().all(&model_use_is_valid))
                && model
                    .complete_read()
                    .roles()
                    .values()
                    .all(|role| role.players().iter().all(&model_use_is_valid));
            let role_upcasts_are_valid =
                model
                    .complete_read()
                    .role_upcasts()
                    .iter()
                    .all(|(active, ancestors)| {
                        model.complete_read().roles().contains_key(active)
                            && ancestors.iter().all(&role_exists)
                    });
            let references_are_valid = model.reference_read().key_fields().iter().all(|id| {
                model
                    .query_tokens()
                    .fields()
                    .get(id)
                    .is_some_and(FieldTokenProjection::is_key)
            });
            let value_subject = model.id().kind() != TypeKind::Attribute
                || model.declaration().value_annotations().keys().all(|id| {
                    id.subject()
                        == &AnnotationSubjectId::Value(ValueFactId::new(
                            AttributeId::new(model.id().label().as_str())
                                .expect("projected attribute label is valid"),
                        ))
                });
            parent_is_valid
                && direct_sub_is_valid
                && direct_fields_are_valid
                && direct_roles_are_valid
                && direct_plays_are_valid
                && fields_are_valid
                && roles_are_valid
                && role_upcasts_are_valid
                && references_are_valid
                && value_subject
        });
        let closed_playing = playing_facts.values().all(|playing| {
            models.contains_key(playing.id().player())
                && role_exists(playing.role())
                && (!matches!(target, BindingTarget::TypeScript | BindingTarget::Rust)
                    || playing.target_name().is_some())
        });
        let closed_functions = functions.values().all(|function| {
            function
                .parameters()
                .iter()
                .all(|parameter| type_ref_is_valid(parameter.type_ref()))
                && match function.returns() {
                    FunctionReturnProjection::Scalar(element) => {
                        type_ref_is_valid(element.type_ref())
                    }
                    FunctionReturnProjection::Tuple(elements)
                    | FunctionReturnProjection::Stream(elements) => elements
                        .iter()
                        .all(|element| type_ref_is_valid(element.type_ref())),
                }
        });
        if !closed_models || !closed_playing || !closed_functions {
            return Err(invalid_projection(
                "invalid_projection_reference",
                "projection graph contains an unavailable type, field, role, specialization, reference, or function dependency",
            ));
        }
        let content = to_canonical_json(&RuntimeProjectionContentView {
            models: &models,
            structs: &structs,
            functions: &functions,
            playing_facts: &playing_facts,
            emission: &emission,
        })?;
        let projection_fingerprint = BindingProjectionFingerprint::compute_with_projection(
            target,
            &semantic_fingerprint,
            &config,
            handlers,
            resources,
            &content,
        )?;
        let mut generator_handlers = handlers.to_vec();
        generator_handlers.sort_by(|left, right| left.id().cmp(right.id()));
        let mut code_resources = resources.to_vec();
        code_resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(Self {
            target,
            config,
            semantic_fingerprint,
            projection_fingerprint,
            generator_handlers,
            code_resources,
            models,
            structs,
            functions,
            playing_facts,
            emission,
        })
    }
    /// Return the binding target.
    #[must_use]
    pub const fn target(&self) -> BindingTarget {
        self.target
    }
    /// Return the exact projection configuration.
    #[must_use]
    pub const fn config(&self) -> &ProjectionConfig {
        &self.config
    }
    /// Return the source semantic schema fingerprint.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        &self.semantic_fingerprint
    }
    /// Return the content-bound target projection fingerprint.
    #[must_use]
    pub const fn projection_fingerprint(&self) -> &BindingProjectionFingerprint {
        &self.projection_fingerprint
    }
    /// Return the ordered handler evidence committed by the projection fingerprint.
    #[must_use]
    pub fn generator_handlers(&self) -> &[ProjectionHandler] {
        &self.generator_handlers
    }
    /// Return the ordered code-resource evidence committed by the projection fingerprint.
    #[must_use]
    pub fn code_resources(&self) -> &[CodeResourceDigest] {
        &self.code_resources
    }
    /// Return projected models in canonical identity order.
    #[must_use]
    pub const fn models(&self) -> &BTreeMap<TypeId, ModelProjection> {
        &self.models
    }
    /// Return projected structs in canonical identity order.
    #[must_use]
    pub const fn structs(&self) -> &BTreeMap<StructId, StructProjection> {
        &self.structs
    }
    /// Return projected schema functions in canonical identity order.
    #[must_use]
    pub const fn functions(&self) -> &BTreeMap<FunctionId, FunctionProjection> {
        &self.functions
    }
    /// Return effective per-player metadata keyed by exact playing identity.
    #[must_use]
    pub const fn playing_facts(&self) -> &BTreeMap<PlaysFactId, PlayingProjection> {
        &self.playing_facts
    }
    /// Return the shells-first generation schedule.
    #[must_use]
    pub const fn emission(&self) -> &EmissionPlan {
        &self.emission
    }
}

impl CodeResourceDigest {
    /// Adopt decoded resource evidence only after checking its exact fingerprint domain.
    pub(crate) fn from_wire(
        id: impl Into<String>,
        content_fingerprint: Fingerprint,
    ) -> Result<Self, Diagnostic> {
        if content_fingerprint.domain().as_str() != CODE_RESOURCE_DOMAIN
            || content_fingerprint.canonicalization().as_str() != RAW_BYTES_CANONICALIZATION
            || content_fingerprint.semantic_profile().is_some()
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::Integrity,
                "invalid_code_resource_fingerprint",
                "code resource fingerprint wire metadata is inconsistent",
            ));
        }
        Ok(Self {
            id: CodeResourceId::new(id)?,
            content_fingerprint,
        })
    }
}
