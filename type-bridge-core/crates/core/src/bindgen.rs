//! Rust-hosted model generation for all supported language targets.
//!
//! This module is the bindgen single source of truth: it derives generation
//! decisions from a resolved [`TypeSchema`](crate::schema::TypeSchema) and
//! renders Python, TypeScript, or Rust source files from the same policy.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::{self, Write};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::schema::{Cardinality, EntityType, OwnedAttribute, RelationType, RoleSpec, TypeSchema};

/// Custom comment annotations keyed by annotation name.
pub type AnnotationMap = BTreeMap<String, Value>;

/// Target language for bindgen rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLanguage {
    /// Render a Python model package.
    Python,
    /// Render a TypeScript model package.
    TypeScript,
    /// Render a Rust model module set.
    Rust,
}

impl fmt::Display for TargetLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Python => f.write_str("python"),
            Self::TypeScript => f.write_str("typescript"),
            Self::Rust => f.write_str("rust"),
        }
    }
}

impl FromStr for TargetLanguage {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "python" | "py" => Ok(Self::Python),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "rust" | "rs" => Ok(Self::Rust),
            other => Err(format!("unsupported bindgen target language: {other}")),
        }
    }
}

/// Python-specific comment metadata consumed by the Rust Python renderer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PythonRenderMetadata {
    /// Entity annotations extracted from schema comments.
    #[serde(default)]
    pub entity_annotations: BTreeMap<String, AnnotationMap>,
    /// Attribute annotations extracted from schema comments.
    #[serde(default)]
    pub attribute_annotations: BTreeMap<String, AnnotationMap>,
    /// Relation annotations extracted from schema comments.
    #[serde(default)]
    pub relation_annotations: BTreeMap<String, AnnotationMap>,
    /// Role annotations extracted from schema comments, keyed relation -> role.
    #[serde(default)]
    pub role_annotations: BTreeMap<String, BTreeMap<String, AnnotationMap>>,
}

/// Options shared by the bindgen renderers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindgenOptions {
    /// Schema version rendered into generated Python package metadata.
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    /// Optional bundled schema filename for generated Python `schema_text()`.
    #[serde(default = "default_schema_filename")]
    pub schema_filename: Option<String>,
    /// Optional source schema text used for generated registry hashing.
    #[serde(default)]
    pub schema_text: Option<String>,
    /// Attribute names that wrappers should promote to generated key fields.
    #[serde(default)]
    pub implicit_key_attributes: Vec<String>,
    /// Python comment metadata that affects generated docstrings and registry metadata.
    #[serde(default)]
    pub python_metadata: PythonRenderMetadata,
}

impl Default for BindgenOptions {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            schema_filename: default_schema_filename(),
            schema_text: None,
            implicit_key_attributes: Vec::new(),
            python_metadata: PythonRenderMetadata::default(),
        }
    }
}

fn default_schema_version() -> String {
    "1.0.0".to_string()
}

fn default_schema_filename() -> Option<String> {
    Some("schema.tql".to_string())
}

/// One generated source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedFile {
    /// Relative output path for the generated file.
    pub path: String,
    /// Complete source text for the generated file.
    pub contents: String,
}

/// A generated package or module set for one target language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedPackage {
    /// The target language this package was rendered for.
    pub target: TargetLanguage,
    /// Files to write, in deterministic output order.
    pub files: Vec<GeneratedFile>,
}

impl GeneratedPackage {
    /// Serialize this generated package as pretty JSON.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    /// Return a generated file by relative path.
    pub fn file(&self, path: &str) -> Option<&GeneratedFile> {
        self.files.iter().find(|file| file.path == path)
    }
}

/// Generated Rust model source files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedRustModels {
    /// `mod.rs` with module declarations.
    pub mod_rs: String,
    /// `attributes.rs` with `define_attribute!` invocations.
    pub attributes_rs: String,
    /// `entities.rs` with entity struct definitions.
    pub entities_rs: String,
    /// `relations.rs` with relation struct definitions.
    pub relations_rs: String,
}

impl GeneratedRustModels {
    /// Convert this Rust-specific file set to a generated package.
    pub fn into_package(self) -> GeneratedPackage {
        GeneratedPackage {
            target: TargetLanguage::Rust,
            files: vec![
                GeneratedFile {
                    path: "mod.rs".to_string(),
                    contents: self.mod_rs,
                },
                GeneratedFile {
                    path: "attributes.rs".to_string(),
                    contents: self.attributes_rs,
                },
                GeneratedFile {
                    path: "entities.rs".to_string(),
                    contents: self.entities_rs,
                },
                GeneratedFile {
                    path: "relations.rs".to_string(),
                    contents: self.relations_rs,
                },
            ],
        }
    }
}

/// Bindgen plan derived from a resolved schema.
#[derive(Debug, Clone)]
pub struct BindgenPlan {
    schema: TypeSchema,
}

impl BindgenPlan {
    /// Build a bindgen plan from an already resolved schema.
    pub fn from_schema(schema: &TypeSchema) -> Self {
        Self {
            schema: schema.clone(),
        }
    }

    /// Parse TypeQL and build a bindgen plan.
    pub fn from_typeql(input: &str) -> Result<Self, String> {
        let schema = TypeSchema::from_typeql(input).map_err(|error| error.to_string())?;
        Ok(Self { schema })
    }

    /// Render this plan for the requested target language.
    pub fn render(&self, target: TargetLanguage, options: &BindgenOptions) -> GeneratedPackage {
        match target {
            TargetLanguage::Python => render_python_package(&self.schema, options),
            TargetLanguage::TypeScript => render_typescript_package(&self.schema, options),
            TargetLanguage::Rust => self.render_rust_models().into_package(),
        }
    }

    /// Render this plan as Rust model source files.
    pub fn render_rust_models(&self) -> GeneratedRustModels {
        render_rust_models(&self.schema, RustRenderMode::Module)
    }

    /// Render this plan as inline Rust source for `include_schema!`.
    pub fn render_rust_inline(&self) -> String {
        let models = render_rust_models(&self.schema, RustRenderMode::Inline);
        [
            models.attributes_rs,
            models.entities_rs,
            models.relations_rs,
        ]
        .join("\n")
    }
}

/// Parse TypeQL and render model files for the requested target.
pub fn generate_from_typeql(
    input: &str,
    target: TargetLanguage,
    options: &BindgenOptions,
) -> Result<GeneratedPackage, String> {
    Ok(BindgenPlan::from_typeql(input)?.render(target, options))
}

/// Parse TypeQL and return generated model files as pretty JSON.
pub fn generate_json_from_typeql(
    input: &str,
    target: TargetLanguage,
    options: &BindgenOptions,
) -> Result<String, String> {
    generate_from_typeql(input, target, options)?.to_json()
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Convert a schema label into the Python class name bindgen emits for it
/// (e.g. `rm-legacy-link` -> `RmLegacyLink`).
///
/// Public so migration authoring can reference generated snapshot symbols by
/// the exact names bindgen will produce for the same schema.
pub fn python_class_name(label: &str) -> String {
    class_name(label)
}

fn class_name(label: &str) -> String {
    label
        .replace('_', "-")
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = first.to_uppercase().collect::<String>();
                    let rest = chars.as_str();
                    let is_upper = part.chars().all(|c| c.is_uppercase());
                    let is_lower = part.chars().all(|c| c.is_lowercase());
                    if is_upper || is_lower {
                        out.push_str(&rest.to_lowercase());
                    } else {
                        out.push_str(rest);
                    }
                    out
                }
                None => String::new(),
            }
        })
        .collect()
}

fn rust_pascal_case(label: &str) -> String {
    label
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = first.to_uppercase().collect::<String>();
                    out.push_str(chars.as_str());
                    out
                }
                None => String::new(),
            }
        })
        .collect()
}

fn field_name(label: &str) -> String {
    label.replace('-', "_")
}

fn python_type_name_case(
    name: &str,
    class: &str,
    default_case: &str,
    annotations: Option<&AnnotationMap>,
) -> Option<String> {
    if let Some(annotations) = annotations
        && let Some(val) = annotations.get("case")
    {
        let mut explicit_case = None;
        if let Some(s) = val.as_str() {
            explicit_case = Some(s.to_string());
        } else if let Some(arr) = val.as_array()
            && arr.len() == 2
            && let (Some(lang), Some(case_val)) = (arr[0].as_str(), arr[1].as_str())
            && lang.to_lowercase() == "python"
        {
            explicit_case = Some(case_val.to_string());
        }
        if let Some(c) = explicit_case {
            let mut c_lower = c.to_lowercase();
            c_lower = c_lower.replace("_", "");
            let mapped_case = match c_lower.as_str() {
                "pascalcase" | "classname" => "CLASS_NAME",
                "snakecase" => "SNAKE_CASE",
                "lowercase" => "LOWERCASE",
                _ => &c, // fallback
            };
            return Some(format!("case=TypeNameCase.{}", mapped_case));
        }
    }

    let lower = class.to_lowercase();
    let mut snake = String::new();
    let chars: Vec<char> = class.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next = if i + 1 < chars.len() {
                Some(chars[i + 1])
            } else {
                None
            };
            let is_prev_lower_or_digit = prev.is_lowercase() || prev.is_ascii_digit();
            let is_next_lower = next.map(|n| n.is_lowercase()).unwrap_or(false);
            if is_prev_lower_or_digit || is_next_lower {
                snake.push('_');
            }
        }
        snake.extend(c.to_lowercase());
    }

    if name == snake {
        if default_case == "SNAKE_CASE" {
            None
        } else {
            Some("case=TypeNameCase.SNAKE_CASE".to_string())
        }
    } else if name == class {
        if default_case == "CLASS_NAME" {
            None
        } else {
            Some("case=TypeNameCase.CLASS_NAME".to_string())
        }
    } else if name == lower {
        if default_case == "LOWERCASE" {
            None
        } else {
            Some("case=TypeNameCase.LOWERCASE".to_string())
        }
    } else {
        Some(format!("name={}", string_literal(name)))
    }
}

fn string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

fn bool_py(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn card_is_optional_single(cardinality: &Cardinality) -> bool {
    cardinality.min == 0 && cardinality.max == Some(1)
}

fn card_is_required_single(cardinality: &Cardinality) -> bool {
    cardinality.min >= 1 && cardinality.max == Some(1)
}

fn card_is_multi(cardinality: &Cardinality) -> bool {
    cardinality.max.is_none_or(|max| max > 1)
}

/// Render `, Doc("...")`, `, Meta("k", "v")` marker arguments for a `Flag(...)`
/// (Python) or `field(...)` (TypeScript) call. Both surfaces share the marker
/// syntax. Returns an empty string when the ownership carries no annotations.
fn doc_meta_flag_args(doc: Option<&str>, meta: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    if let Some(doc) = doc {
        write!(out, ", Doc({})", string_literal(doc)).unwrap();
    }
    for (key, value) in meta {
        write!(
            out,
            ", Meta({}, {})",
            string_literal(key),
            string_literal(value)
        )
        .unwrap();
    }
    out
}

/// Render a Python dict literal `{"key": "value", ...}` for `meta=` kwargs.
fn py_meta_literal(meta: &BTreeMap<String, String>) -> String {
    let entries = meta
        .iter()
        .map(|(key, value)| format!("{}: {}", string_literal(key), string_literal(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{entries}}}")
}

/// Render a TypeScript object literal `{ "key": "value", ... }` for `meta:` options.
fn ts_meta_literal(meta: &BTreeMap<String, String>) -> String {
    let entries = meta
        .iter()
        .map(|(key, value)| format!("{}: {}", string_literal(key), string_literal(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {entries} }}")
}

fn card_expr(cardinality: &Cardinality) -> String {
    match cardinality.max {
        Some(max) => format!("Card({}, {})", cardinality.min, max),
        None => format!("Card({})", cardinality.min),
    }
}

fn ts_card_expr(cardinality: &Cardinality) -> String {
    match cardinality.max {
        Some(max) => format!("Card({}, {})", cardinality.min, max),
        None => format!("Card({}, null)", cardinality.min),
    }
}

fn topological_sort<T, F>(map: &BTreeMap<String, T>, get_parent: F) -> Vec<String>
where
    F: Fn(&T) -> Option<&str>,
{
    let mut result = Vec::new();
    let mut visited = HashSet::new();

    fn visit<T, F>(
        name: &str,
        map: &BTreeMap<String, T>,
        get_parent: &F,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) where
        F: Fn(&T) -> Option<&str>,
    {
        if visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());
        if let Some(value) = map.get(name)
            && let Some(parent) = get_parent(value)
        {
            visit(parent, map, get_parent, visited, result);
        }
        result.push(name.to_string());
    }

    for name in map.keys() {
        visit(name, map, &get_parent, &mut visited, &mut result);
    }
    result
}

fn ordered_owned_attributes<'a>(
    owns: &'a [OwnedAttribute],
    order: &[String],
) -> Vec<&'a OwnedAttribute> {
    if order.is_empty() {
        return owns.iter().collect();
    }
    let mut result = Vec::new();
    for name in order {
        if let Some(owned) = owns.iter().find(|owned| owned.name == *name) {
            result.push(owned);
        }
    }
    result
}

fn direct_parent_owns<'a>(
    schema: &'a TypeSchema,
    parent: Option<&String>,
    relation: bool,
) -> BTreeSet<&'a str> {
    match (relation, parent) {
        (false, Some(parent_name)) => schema
            .entities
            .get(parent_name)
            .map(|entity| {
                entity
                    .owns
                    .iter()
                    .map(|owned| owned.name.as_str())
                    .collect()
            })
            .unwrap_or_default(),
        (true, Some(parent_name)) => schema
            .relations
            .get(parent_name)
            .map(|relation| {
                relation
                    .owns
                    .iter()
                    .map(|owned| owned.name.as_str())
                    .collect()
            })
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    }
}

fn direct_parent_roles<'a>(schema: &'a TypeSchema, parent: Option<&String>) -> BTreeSet<&'a str> {
    parent
        .and_then(|parent_name| schema.relations.get(parent_name))
        .map(|relation| {
            relation
                .roles
                .iter()
                .map(|role| role.name.as_str())
                .collect()
        })
        .unwrap_or_default()
}

fn resolved_attr_value_type<'a>(schema: &'a TypeSchema, attr_name: &str) -> &'a str {
    let mut current = attr_name;
    let mut visited = HashSet::new();
    while visited.insert(current.to_string()) {
        let Some(attr) = schema.attributes.get(current) else {
            return "string";
        };
        if !attr.value_type.is_empty() {
            return attr.value_type.as_str();
        }
        let Some(parent) = attr.parent.as_deref() else {
            return "string";
        };
        current = parent;
    }
    "string"
}

fn python_value_base(value_type: &str) -> &'static str {
    match value_type {
        "string" => "String",
        "integer" | "int" | "long" => "Integer",
        "double" => "Double",
        "boolean" | "bool" => "Boolean",
        "date" => "Date",
        "datetime" => "DateTime",
        "datetime-tz" => "DateTimeTZ",
        "decimal" => "Decimal",
        "duration" => "Duration",
        _ => "String",
    }
}

fn ts_attr_kind(value_type: &str) -> &'static str {
    match value_type {
        "string" => "String",
        "integer" | "int" | "long" => "Integer",
        "double" => "Double",
        "boolean" | "bool" => "Boolean",
        "date" => "Date",
        "datetime" => "DateTime",
        "datetime-tz" => "DateTimeTZ",
        "decimal" => "Decimal",
        "duration" => "Duration",
        _ => "String",
    }
}

fn rust_value_type(value_type: &str) -> &str {
    match value_type {
        "integer" | "int" | "long" => "long",
        "boolean" | "bool" => "boolean",
        other => other,
    }
}

fn annotation_without_docstring(
    metadata: &BTreeMap<String, AnnotationMap>,
    name: &str,
) -> AnnotationMap {
    let mut annotations = metadata.get(name).cloned().unwrap_or_default();
    annotations.remove("_docstring");
    annotations
}

fn docstring(metadata: &BTreeMap<String, AnnotationMap>, name: &str) -> Option<String> {
    metadata
        .get(name)
        .and_then(|annotations| annotations.get("_docstring"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn py_annotation_literal(value: &Value) -> String {
    match value {
        Value::Bool(value) => bool_py(*value).to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => string_literal(value),
        Value::Array(values) => {
            let entries = values
                .iter()
                .map(py_annotation_literal)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{}]", entries)
        }
        Value::Null => "None".to_string(),
        Value::Object(_) => string_literal(&value.to_string()),
    }
}

fn py_range_literal(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "None".to_string();
    };
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        value.to_string()
    } else {
        string_literal(value)
    }
}

fn minimal_role_players(schema: &TypeSchema, relation_name: &str, role_name: &str) -> Vec<String> {
    let role_ref = format!("{relation_name}:{role_name}");
    let players = schema
        .entities
        .iter()
        .filter(|(_, entity)| {
            entity
                .plays
                .iter()
                .any(|played| played.role_ref == role_ref)
        })
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();

    if players.is_empty() {
        return Vec::new();
    }

    let parent_map = schema
        .entities
        .iter()
        .map(|(name, entity)| (name.as_str(), entity.parent.as_deref()))
        .collect::<BTreeMap<_, _>>();

    fn is_ancestor(
        parent_map: &BTreeMap<&str, Option<&str>>,
        candidate: &str,
        target: &str,
    ) -> bool {
        let mut current = parent_map.get(target).and_then(|parent| *parent);
        while let Some(parent) = current {
            if parent == candidate {
                return true;
            }
            current = parent_map.get(parent).and_then(|next| *next);
        }
        false
    }

    let mut minimal = players.clone();
    for player in &players {
        for other in &players {
            if player != other && is_ancestor(&parent_map, other, player) {
                minimal.remove(player);
                break;
            }
        }
    }

    minimal.into_iter().collect()
}

fn plays_cardinality_for_role(
    schema: &TypeSchema,
    relation_name: &str,
    role_name: &str,
    players: &[String],
) -> Option<Cardinality> {
    let role_ref = format!("{relation_name}:{role_name}");
    for player in players {
        let Some(entity) = schema.entities.get(player) else {
            continue;
        };
        if let Some(cardinality) = entity
            .plays
            .iter()
            .find(|played| played.role_ref == role_ref)
            .and_then(|played| played.cardinality.clone())
        {
            return Some(cardinality);
        }
    }
    None
}

fn schema_hash(schema_text: Option<&str>) -> String {
    let Some(schema_text) = schema_text else {
        return String::new();
    };
    let digest = Sha256::digest(schema_text.as_bytes());
    format!("sha256:{:x}", digest)[..23].to_string()
}

// ---------------------------------------------------------------------------
// Python renderer
// ---------------------------------------------------------------------------

fn render_python_package(schema: &TypeSchema, options: &BindgenOptions) -> GeneratedPackage {
    let functions = render_python_functions(schema, options);
    let structs = render_python_structs(schema, options);
    let functions_present = functions.is_some();

    let mut files = vec![
        GeneratedFile {
            path: "attributes.py".to_string(),
            contents: render_python_attributes(schema, options),
        },
        GeneratedFile {
            path: "entities.py".to_string(),
            contents: render_python_entities(schema, options),
        },
        GeneratedFile {
            path: "relations.py".to_string(),
            contents: render_python_relations(schema, options),
        },
        GeneratedFile {
            path: "registry.py".to_string(),
            contents: render_python_registry(schema, options, options.schema_text.as_deref()),
        },
    ];
    if let Some(functions) = functions {
        files.push(GeneratedFile {
            path: "functions.py".to_string(),
            contents: functions,
        });
    }
    if let Some(structs) = structs {
        files.push(GeneratedFile {
            path: "structs.py".to_string(),
            contents: structs,
        });
    }
    files.push(GeneratedFile {
        path: "__init__.py".to_string(),
        contents: render_python_package_init(schema, options, functions_present),
    });

    GeneratedPackage {
        target: TargetLanguage::Python,
        files,
    }
}

fn render_python_attributes(schema: &TypeSchema, options: &BindgenOptions) -> String {
    let order = topological_sort(&schema.attributes, |attr| attr.parent.as_deref());
    let uses_classvar = schema.attributes.values().any(|attr| {
        attr.allowed_values.is_some()
            || attr.regex.is_some()
            || attr.range_min.is_some()
            || attr.range_max.is_some()
    });

    let mut imports = BTreeSet::from(["AttributeFlags".to_string(), "TypeNameCase".to_string()]);
    for name in &order {
        let attr = &schema.attributes[name];
        let base = if let Some(parent) = attr.parent.as_deref() {
            if schema.attributes.contains_key(parent) {
                class_name(parent)
            } else {
                python_value_base(resolved_attr_value_type(schema, name)).to_string()
            }
        } else {
            python_value_base(resolved_attr_value_type(schema, name)).to_string()
        };
        if matches!(
            base.as_str(),
            "String"
                | "Integer"
                | "Double"
                | "Boolean"
                | "Date"
                | "DateTime"
                | "DateTimeTZ"
                | "Decimal"
                | "Duration"
        ) {
            imports.insert(base);
        }
    }

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"Attribute type definitions generated from a TypeDB schema.\n\nAUTO-GENERATED FILE - DO NOT EDIT MANUALLY\nRegenerate with: type-bridge generate <schema.tql> <output_dir>\n\"\"\""
    )
    .unwrap();
    if uses_classvar {
        writeln!(out).unwrap();
        writeln!(out, "from typing import ClassVar").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(
        out,
        "from type_bridge import {}",
        imports.into_iter().collect::<Vec<_>>().join(", ")
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();

    for name in &order {
        let attr = &schema.attributes[name];
        let class = class_name(name);
        let base = attr
            .parent
            .as_deref()
            .filter(|parent| schema.attributes.contains_key(*parent))
            .map(class_name)
            .unwrap_or_else(|| {
                python_value_base(resolved_attr_value_type(schema, name)).to_string()
            });
        let doc = attr
            .doc
            .clone()
            .or_else(|| docstring(&options.python_metadata.attribute_annotations, name))
            .unwrap_or_else(|| format!("Attribute for `{name}`."));

        writeln!(out, "class {class}({base}):").unwrap();
        writeln!(out, "    \"\"\"{doc}\"\"\"").unwrap();
        let mut flags = Vec::new();
        if let Some(case) = python_type_name_case(
            name,
            &class,
            "SNAKE_CASE",
            options.python_metadata.attribute_annotations.get(name),
        ) {
            flags.push(case);
        }
        if !flags.iter().any(|f| f.starts_with("name=")) {
            flags.insert(0, format!("name={}", string_literal(name)));
        }
        if let Some(doc_text) = attr.doc.as_deref() {
            flags.push(format!("doc={}", string_literal(doc_text)));
        }
        if !attr.meta.is_empty() {
            flags.push(format!("meta={}", py_meta_literal(&attr.meta)));
        }
        let flags_str = if flags.is_empty() {
            String::new()
        } else {
            flags.join(", ")
        };
        writeln!(out, "    flags = AttributeFlags({flags_str})").unwrap();
        if attr.is_independent {
            writeln!(out, "    independent = True").unwrap();
        }
        if let Some(regex) = attr.regex.as_deref() {
            writeln!(out, "    regex: ClassVar[str] = r{}", string_literal(regex)).unwrap();
        }
        if let Some(values) = attr.allowed_values.as_ref() {
            let values = values
                .iter()
                .map(|value| string_literal(value))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                out,
                "    allowed_values: ClassVar[tuple[str, ...]] = ({values},)"
            )
            .unwrap();
        }
        if attr.range_min.is_some() || attr.range_max.is_some() {
            writeln!(
                out,
                "    range_constraint: ClassVar[tuple[int | float | None, int | float | None]] = ({}, {})",
                py_range_literal(attr.range_min.as_deref()),
                py_range_literal(attr.range_max.as_deref())
            )
            .unwrap();
        }
        writeln!(out).unwrap();
        writeln!(out).unwrap();
    }

    writeln!(out, "__all__ = [").unwrap();
    for name in order
        .iter()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "]").unwrap();
    out
}

fn render_python_attr_field(owned: &OwnedAttribute, attr_class: &str, is_key: bool) -> String {
    let py_name = field_name(&owned.name);
    let mut extras = doc_meta_flag_args(owned.doc.as_deref(), &owned.meta);
    if is_key {
        return format!("{py_name}: attributes.{attr_class} = Flag(Key{extras})");
    }
    // @unique does not imply required: unlike @key it keeps the default
    // card(0..1), so it composes with the cardinality handling below as a
    // plain marker instead of short-circuiting the optionality logic.
    if owned.is_unique {
        extras = format!(", Unique{extras}");
    }
    if owned.ordered {
        if owned.distinct {
            return format!(
                "{py_name}: list[attributes.{attr_class}] = Flag(Ordered, Distinct{extras})"
            );
        }
        return format!("{py_name}: list[attributes.{attr_class}] = Flag(Ordered{extras})");
    }
    // For single-valued ownerships the optionality lives in the type annotation;
    // a Flag(...) default is only emitted when annotation markers require one.
    let bare_extras = extras.trim_start_matches(", ");
    match owned.cardinality.as_ref() {
        Some(cardinality) if card_is_multi(cardinality) => match cardinality.max {
            Some(max) => format!(
                "{py_name}: list[attributes.{attr_class}] = Flag(Card({}, {}){extras})",
                cardinality.min, max
            ),
            None => format!(
                "{py_name}: list[attributes.{attr_class}] = Flag(Card(min={}){extras})",
                cardinality.min
            ),
        },
        Some(cardinality) if card_is_required_single(cardinality) => {
            if extras.is_empty() {
                format!("{py_name}: attributes.{attr_class}")
            } else {
                format!("{py_name}: attributes.{attr_class} = Flag({bare_extras})")
            }
        }
        // None or @card(0..1): optional single value.
        _ => {
            if extras.is_empty() {
                format!("{py_name}: attributes.{attr_class} | None = None")
            } else {
                format!("{py_name}: attributes.{attr_class} | None = Flag({bare_extras})")
            }
        }
    }
}

fn render_python_entities(schema: &TypeSchema, options: &BindgenOptions) -> String {
    let order = topological_sort(&schema.entities, |entity| entity.parent.as_deref());
    let implicit_keys = options
        .implicit_key_attributes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let needs_card = schema
        .entities
        .values()
        .flat_map(|entity| entity.owns.iter())
        .filter_map(|owned| owned.cardinality.as_ref())
        .any(card_is_multi);
    let needs_ordered = schema
        .entities
        .values()
        .any(|entity| entity.owns.iter().any(|owned| owned.ordered));
    let needs_distinct = schema
        .entities
        .values()
        .any(|entity| entity.owns.iter().any(|owned| owned.distinct));
    let needs_doc = schema
        .entities
        .values()
        .any(|entity| entity.owns.iter().any(|owned| owned.doc.is_some()));
    let needs_meta = schema
        .entities
        .values()
        .any(|entity| entity.owns.iter().any(|owned| !owned.meta.is_empty()));

    let mut imports = vec![
        "Entity",
        "Flag",
        "Key",
        "TypeFlags",
        "TypeNameCase",
        "Unique",
    ];
    if needs_card {
        imports.insert(1, "Card");
    }
    if needs_distinct {
        imports.push("Distinct");
    }
    if needs_doc {
        imports.push("Doc");
    }
    if needs_meta {
        imports.push("Meta");
    }
    if needs_ordered {
        imports.push("Ordered");
    }

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"Entity type definitions generated from a TypeDB schema.\n\nAUTO-GENERATED FILE - DO NOT EDIT MANUALLY\nRegenerate with: type-bridge generate <schema.tql> <output_dir>\n\"\"\"\n"
    )
    .unwrap();
    writeln!(out, "from typing import ClassVar\n").unwrap();
    writeln!(out, "from type_bridge import {}", imports.join(", ")).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "from . import attributes").unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();

    for name in &order {
        let entity = &schema.entities[name];
        let class = class_name(name);
        let base = entity
            .parent
            .as_deref()
            .filter(|parent| schema.entities.contains_key(*parent))
            .map(class_name)
            .unwrap_or_else(|| "Entity".to_string());
        let doc = entity
            .doc
            .clone()
            .or_else(|| docstring(&options.python_metadata.entity_annotations, name))
            .unwrap_or_else(|| format!("Entity generated from `{name}`."));
        let mut flags = Vec::new();
        if let Some(case) = python_type_name_case(
            name,
            &class,
            "CLASS_NAME",
            options
                .python_metadata
                .entity_annotations
                .get(name)
                .or_else(|| options.python_metadata.relation_annotations.get(name)),
        ) {
            flags.push(case);
        }
        if entity.is_abstract {
            flags.push("abstract=True".to_string());
        }
        if let Some(doc_text) = entity.doc.as_deref() {
            flags.push(format!("doc={}", string_literal(doc_text)));
        }
        if !entity.meta.is_empty() {
            flags.push(format!("meta={}", py_meta_literal(&entity.meta)));
        }

        writeln!(out, "class {class}({base}):").unwrap();
        writeln!(out, "    \"\"\"{doc}\"\"\"").unwrap();
        writeln!(out, "    flags = TypeFlags({})", flags.join(", ")).unwrap();

        if !entity.plays.is_empty() {
            let plays = entity
                .plays
                .iter()
                .map(|played| played.role_ref.as_str())
                .collect::<BTreeSet<_>>();
            if plays.len() == 1 {
                let play = plays.iter().next().unwrap();
                writeln!(out, "    plays: ClassVar[tuple[str, ...]] = (\"{play}\",)").unwrap();
            } else {
                writeln!(out, "    plays: ClassVar[tuple[str, ...]] = (").unwrap();
                for play in plays {
                    writeln!(out, "        \"{play}\",").unwrap();
                }
                writeln!(out, "    )").unwrap();
            }
        }

        let parent_owns = direct_parent_owns(schema, entity.parent.as_ref(), false);
        for owned in ordered_owned_attributes(&entity.owns, &entity.owns_order) {
            if parent_owns.contains(owned.name.as_str()) {
                continue;
            }
            if !schema.attributes.contains_key(&owned.name) {
                continue;
            }
            let is_key = owned.is_key || implicit_keys.contains(owned.name.as_str());
            let field = render_python_attr_field(owned, &class_name(&owned.name), is_key);
            writeln!(out, "    {field}").unwrap();
        }

        let cascades = entity
            .owns
            .iter()
            .filter(|owned| owned.is_cascade)
            .map(|owned| owned.name.as_str())
            .collect::<BTreeSet<_>>();
        if !cascades.is_empty() {
            writeln!(
                out,
                "    # TODO: @cascade annotation (coming soon in TypeDB)"
            )
            .unwrap();
            writeln!(
                out,
                "    # cascade_attrs: [{}]",
                cascades
                    .iter()
                    .map(|name| format!("'{name}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
        }

        writeln!(out).unwrap();
        writeln!(out).unwrap();
    }

    writeln!(out, "__all__ = [").unwrap();
    for name in order
        .iter()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "]").unwrap();
    out
}

fn render_python_role_field(
    role: &RoleSpec,
    player_classes: &[String],
    plays_cardinality: Option<&Cardinality>,
) -> Option<String> {
    if player_classes.is_empty() {
        return None;
    }
    let py_name = field_name(&role.name);
    let mut args = Vec::new();
    if let Some(cardinality) = role.cardinality.as_ref()
        && !(cardinality.min == 1 && cardinality.max == Some(1))
    {
        args.push(format!(", cardinality={}", card_expr(cardinality)));
    }
    if let Some(cardinality) = plays_cardinality {
        args.push(format!(", plays_cardinality={}", card_expr(cardinality)));
    }
    if let Some(overrides) = role.overrides.as_deref() {
        args.push(format!(", overrides={}", string_literal(overrides)));
    }
    if role.is_abstract {
        args.push(", abstract=True".to_string());
    }
    if role.ordered {
        args.push(", ordered=True".to_string());
    }
    if role.distinct {
        args.push(", distinct=True".to_string());
    }
    if let Some(doc_text) = role.doc.as_deref() {
        args.push(format!(", doc={}", string_literal(doc_text)));
    }
    if !role.meta.is_empty() {
        args.push(format!(", meta={}", py_meta_literal(&role.meta)));
    }
    let args = args.join("");

    if player_classes.len() == 1 {
        let player = &player_classes[0];
        return Some(format!(
            "{py_name}: Role[entities.{player}] = Role({}, entities.{player}{args})",
            string_literal(&role.name)
        ));
    }

    let primary = &player_classes[0];
    let extras = player_classes[1..]
        .iter()
        .map(|player| format!("entities.{player}"))
        .collect::<Vec<_>>()
        .join(", ");
    let union_type = player_classes
        .iter()
        .map(|player| format!("entities.{player}"))
        .collect::<Vec<_>>()
        .join(" | ");
    Some(format!(
        "{py_name}: Role[{union_type}] = _multi(Role.multi({}, entities.{primary}, {extras}{args}))",
        string_literal(&role.name)
    ))
}

fn render_python_relations(schema: &TypeSchema, options: &BindgenOptions) -> String {
    let order = topological_sort(&schema.relations, |relation| relation.parent.as_deref());
    let implicit_keys = options
        .implicit_key_attributes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    let needs_card = schema.relations.values().any(|relation| {
        relation
            .owns
            .iter()
            .filter_map(|owned| owned.cardinality.as_ref())
            .any(card_is_multi)
            || relation.roles.iter().any(|role| {
                role.cardinality.as_ref().is_some_and(|cardinality| {
                    !(cardinality.min == 1 && cardinality.max == Some(1))
                })
            })
            || relation.roles.iter().any(|role| {
                let players = minimal_role_players(schema, &relation.name, &role.name);
                plays_cardinality_for_role(schema, &relation.name, &role.name, &players).is_some()
            })
    });
    let needs_key = schema.relations.values().any(|relation| {
        relation
            .owns
            .iter()
            .any(|owned| owned.is_key || implicit_keys.contains(owned.name.as_str()))
    });
    let needs_unique = schema
        .relations
        .values()
        .any(|relation| relation.owns.iter().any(|owned| owned.is_unique));
    let needs_ordered = schema.relations.values().any(|relation| {
        relation.roles.iter().any(|role| role.ordered)
            || relation.owns.iter().any(|owned| owned.ordered)
    });
    let needs_distinct = schema.relations.values().any(|relation| {
        relation.roles.iter().any(|role| role.distinct)
            || relation.owns.iter().any(|owned| owned.distinct)
    });
    let needs_doc = schema
        .relations
        .values()
        .any(|relation| relation.owns.iter().any(|owned| owned.doc.is_some()));
    let needs_meta = schema
        .relations
        .values()
        .any(|relation| relation.owns.iter().any(|owned| !owned.meta.is_empty()));

    let mut imports = vec!["Relation", "Role", "TypeFlags", "TypeNameCase"];
    if needs_card {
        imports.insert(0, "Card");
    }
    if needs_key || needs_unique || needs_card || needs_ordered || needs_doc || needs_meta {
        imports.insert(0, "Flag");
    }
    if needs_key {
        imports.push("Key");
    }
    if needs_unique {
        imports.push("Unique");
    }
    if needs_distinct {
        imports.push("Distinct");
    }
    if needs_doc {
        imports.push("Doc");
    }
    if needs_meta {
        imports.push("Meta");
    }
    if needs_ordered {
        imports.push("Ordered");
    }

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"Relation type definitions generated from a TypeDB schema.\n\nAUTO-GENERATED FILE - DO NOT EDIT MANUALLY\nRegenerate with: type-bridge generate <schema.tql> <output_dir>\n\"\"\"\n"
    )
    .unwrap();
    writeln!(out, "from type_bridge import {}", imports.join(", ")).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "from . import attributes, entities").unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "def _multi(role: Role) -> Role:").unwrap();
    writeln!(
        out,
        "    \"\"\"Attach allowed_player_types for compatibility with MultiRole.\"\"\""
    )
    .unwrap();
    writeln!(
        out,
        "    role.allowed_player_types = role.player_entity_types"
    )
    .unwrap();
    writeln!(out, "    return role").unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();

    for name in &order {
        let relation = &schema.relations[name];
        let class = class_name(name);
        let base = relation
            .parent
            .as_deref()
            .filter(|parent| schema.relations.contains_key(*parent))
            .map(class_name)
            .unwrap_or_else(|| "Relation".to_string());
        let doc = relation
            .doc
            .clone()
            .or_else(|| docstring(&options.python_metadata.relation_annotations, name))
            .unwrap_or_else(|| format!("Relation generated from `{name}`."));
        let mut flags = Vec::new();
        if let Some(case) = python_type_name_case(
            name,
            &class,
            "CLASS_NAME",
            options
                .python_metadata
                .entity_annotations
                .get(name)
                .or_else(|| options.python_metadata.relation_annotations.get(name)),
        ) {
            flags.push(case);
        }
        if relation.is_abstract {
            flags.push("abstract=True".to_string());
        }
        if let Some(doc_text) = relation.doc.as_deref() {
            flags.push(format!("doc={}", string_literal(doc_text)));
        }
        if !relation.meta.is_empty() {
            flags.push(format!("meta={}", py_meta_literal(&relation.meta)));
        }

        writeln!(out, "class {class}({base}):").unwrap();
        writeln!(out, "    \"\"\"{doc}\"\"\"").unwrap();
        writeln!(out, "    flags = TypeFlags({})", flags.join(", ")).unwrap();

        let parent_owns = direct_parent_owns(schema, relation.parent.as_ref(), true);
        for owned in ordered_owned_attributes(&relation.owns, &relation.owns_order) {
            if parent_owns.contains(owned.name.as_str()) {
                continue;
            }
            if !schema.attributes.contains_key(&owned.name) {
                continue;
            }
            let is_key = owned.is_key || implicit_keys.contains(owned.name.as_str());
            let field = render_python_attr_field(owned, &class_name(&owned.name), is_key);
            writeln!(out, "    {field}").unwrap();
        }

        let parent_roles = direct_parent_roles(schema, relation.parent.as_ref());
        for role in &relation.roles {
            if parent_roles.contains(role.name.as_str()) && role.overrides.is_none() {
                continue;
            }
            let players = minimal_role_players(schema, name, &role.name);
            let player_classes = players
                .iter()
                .map(|player| class_name(player))
                .collect::<Vec<_>>();
            let plays_cardinality = plays_cardinality_for_role(schema, name, &role.name, &players);
            if let Some(role_line) =
                render_python_role_field(role, &player_classes, plays_cardinality.as_ref())
            {
                writeln!(out, "    {role_line}").unwrap();
            }
        }

        let annotations =
            annotation_without_docstring(&options.python_metadata.relation_annotations, name);
        if !annotations.is_empty() {
            writeln!(out, "    # Custom annotations from schema:").unwrap();
            for (annotation, value) in annotations {
                if value == Value::Bool(true) {
                    writeln!(out, "    # @{annotation}").unwrap();
                } else {
                    writeln!(
                        out,
                        "    # @{annotation}({})",
                        py_annotation_literal(&value)
                    )
                    .unwrap();
                }
                writeln!(out).unwrap();
            }
        }

        writeln!(out).unwrap();
        writeln!(out).unwrap();
    }

    writeln!(
        out,
        "def get_roles(relation_cls: type[Relation]) -> dict[str, Role]:"
    )
    .unwrap();
    writeln!(
        out,
        "    \"\"\"Expose relation roles for introspection.\"\"\""
    )
    .unwrap();
    writeln!(out, "    return relation_cls.get_roles()").unwrap();
    writeln!(out).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "__all__ = [").unwrap();
    for name in order
        .iter()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "    \"get_roles\",").unwrap();
    writeln!(out, "]").unwrap();
    out
}

fn render_python_registry(
    schema: &TypeSchema,
    options: &BindgenOptions,
    schema_text: Option<&str>,
) -> String {
    let entity_names = schema.entities.keys().cloned().collect::<Vec<_>>();
    let relation_names = schema.relations.keys().cloned().collect::<Vec<_>>();
    let attribute_names = schema.attributes.keys().cloned().collect::<Vec<_>>();

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"TypeBridge registry - Pre-computed schema metadata.\n\nThis module provides static, type-safe access to schema information\nwithout runtime introspection. All data is computed at generation time.\n\nAUTO-GENERATED FILE - DO NOT EDIT MANUALLY\nRegenerate with: type-bridge generate <schema.tql> <output_dir>\n\"\"\"\n"
    )
    .unwrap();
    writeln!(out, "from __future__ import annotations\n").unwrap();
    writeln!(out, "from dataclasses import dataclass").unwrap();
    writeln!(out, "from enum import StrEnum").unwrap();
    writeln!(out, "from typing import TYPE_CHECKING\n").unwrap();
    writeln!(out, "from . import attributes, entities, relations\n").unwrap();
    writeln!(out, "if TYPE_CHECKING:").unwrap();
    writeln!(
        out,
        "    from type_bridge import Attribute, Entity, Relation\n"
    )
    .unwrap();
    writeln!(
        out,
        "SCHEMA_VERSION: str = {}",
        string_literal(&options.schema_version)
    )
    .unwrap();
    writeln!(
        out,
        "SCHEMA_HASH: str = {}",
        string_literal(&schema_hash(schema_text))
    )
    .unwrap();
    writeln!(out).unwrap();

    write_py_tuple(&mut out, "ENTITY_TYPES", &entity_names);
    write_py_tuple(&mut out, "RELATION_TYPES", &relation_names);
    write_py_tuple(&mut out, "ATTRIBUTE_TYPES", &attribute_names);

    write_py_enum(&mut out, "EntityType", "entity", &entity_names);
    write_py_enum(&mut out, "RelationType", "relation", &relation_names);
    write_py_enum(&mut out, "AttributeType", "attribute", &attribute_names);

    write_py_class_map(&mut out, "ENTITY_MAP", "Entity", "entities", &entity_names);
    write_py_class_map(
        &mut out,
        "RELATION_MAP",
        "Relation",
        "relations",
        &relation_names,
    );
    write_py_class_map(
        &mut out,
        "ATTRIBUTE_MAP",
        "Attribute",
        "attributes",
        &attribute_names,
    );

    writeln!(out, "@dataclass(frozen=True, slots=True)").unwrap();
    writeln!(out, "class RoleInfo:").unwrap();
    writeln!(out, "    \"\"\"Metadata for a relation role.\"\"\"\n").unwrap();
    writeln!(out, "    role_name: str").unwrap();
    writeln!(out, "    player_types: tuple[str, ...]\n").unwrap();

    writeln!(out, "RELATION_ROLES: dict[str, dict[str, RoleInfo]] = {{").unwrap();
    for rel_name in &relation_names {
        let relation = &schema.relations[rel_name];
        if relation.roles.is_empty() {
            continue;
        }
        writeln!(out, "    \"{rel_name}\": {{").unwrap();
        for role in &relation.roles {
            let players = minimal_role_players(schema, rel_name, &role.name);
            writeln!(
                out,
                "        \"{}\": RoleInfo({}, {}),",
                role.name,
                string_literal(&role.name),
                py_tuple_literal(&players)
            )
            .unwrap();
        }
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "}}\n").unwrap();

    write_py_attr_sets(&mut out, "RELATION_ATTRIBUTES", &relation_names, |name| {
        schema.relations[name]
            .owns
            .iter()
            .map(|owned| owned.name.clone())
            .collect()
    });
    write_py_attr_sets(&mut out, "ENTITY_ATTRIBUTES", &entity_names, |name| {
        schema.entities[name]
            .owns
            .iter()
            .map(|owned| owned.name.clone())
            .collect()
    });
    write_py_attr_sets(&mut out, "ENTITY_KEYS", &entity_names, |name| {
        schema.entities[name]
            .owns
            .iter()
            .filter(|owned| owned.is_key)
            .map(|owned| owned.name.clone())
            .collect()
    });

    writeln!(out, "ATTRIBUTE_VALUE_TYPES: dict[str, str] = {{").unwrap();
    for name in &attribute_names {
        let attr = &schema.attributes[name];
        if !attr.value_type.is_empty() {
            writeln!(out, "    \"{name}\": \"{}\",", attr.value_type).unwrap();
        }
    }
    writeln!(out, "}}\n").unwrap();

    write_py_parent_map(&mut out, "ENTITY_PARENTS", &entity_names, |name| {
        schema.entities[name].parent.clone()
    });
    write_py_parent_map(&mut out, "RELATION_PARENTS", &relation_names, |name| {
        schema.relations[name].parent.clone()
    });

    write_py_frozenset(
        &mut out,
        "ENTITY_ABSTRACT",
        &entity_names
            .iter()
            .filter(|name| schema.entities[*name].is_abstract)
            .cloned()
            .collect::<Vec<_>>(),
    );
    write_py_frozenset(
        &mut out,
        "RELATION_ABSTRACT",
        &relation_names
            .iter()
            .filter(|name| schema.relations[*name].is_abstract)
            .cloned()
            .collect::<Vec<_>>(),
    );

    write_py_annotations(
        &mut out,
        "ENTITY_ANNOTATIONS",
        &options.python_metadata.entity_annotations,
    );
    write_py_annotations(
        &mut out,
        "ATTRIBUTE_ANNOTATIONS",
        &options.python_metadata.attribute_annotations,
    );
    write_py_annotations(
        &mut out,
        "RELATION_ANNOTATIONS",
        &options.python_metadata.relation_annotations,
    );

    writeln!(
        out,
        "ENTITY_TYPE_JSON_SCHEMA: dict = {{\n    \"type\": \"string\",\n    \"enum\": list(ENTITY_TYPES),\n    \"description\": \"Valid entity type names\",\n}}\n"
    )
    .unwrap();
    writeln!(
        out,
        "RELATION_TYPE_JSON_SCHEMA: dict = {{\n    \"type\": \"string\",\n    \"enum\": list(RELATION_TYPES),\n    \"description\": \"Valid relation type names\",\n}}\n"
    )
    .unwrap();
    writeln!(
        out,
        "ATTRIBUTE_TYPE_JSON_SCHEMA: dict = {{\n    \"type\": \"string\",\n    \"enum\": list(ATTRIBUTE_TYPES),\n    \"description\": \"Valid attribute type names\",\n}}\n"
    )
    .unwrap();

    writeln!(
        out,
        "def get_entity_class(type_name: str) -> type[\"Entity\"] | None:\n    \"\"\"Get entity class by TypeDB type name.\"\"\"\n    return ENTITY_MAP.get(type_name)\n"
    )
    .unwrap();
    writeln!(
        out,
        "def get_relation_class(type_name: str) -> type[\"Relation\"] | None:\n    \"\"\"Get relation class by TypeDB type name.\"\"\"\n    return RELATION_MAP.get(type_name)\n"
    )
    .unwrap();
    writeln!(
        out,
        "def get_attribute_class(type_name: str) -> type[\"Attribute\"] | None:\n    \"\"\"Get attribute class by TypeDB type name.\"\"\"\n    return ATTRIBUTE_MAP.get(type_name)\n"
    )
    .unwrap();
    writeln!(
        out,
        "def get_role_players(relation_type: str, role_name: str) -> tuple[str, ...]:\n    \"\"\"Get allowed player entity types for a relation role.\"\"\"\n    roles = RELATION_ROLES.get(relation_type, {{}})\n    role_info = roles.get(role_name)\n    return role_info.player_types if role_info else ()\n"
    )
    .unwrap();
    writeln!(
        out,
        "def get_relation_attributes(relation_type: str) -> frozenset[str]:\n    \"\"\"Get attribute names owned by a relation type.\"\"\"\n    return RELATION_ATTRIBUTES.get(relation_type, frozenset())\n"
    )
    .unwrap();
    writeln!(
        out,
        "def get_entity_attributes(entity_type: str) -> frozenset[str]:\n    \"\"\"Get attribute names owned by an entity type.\"\"\"\n    return ENTITY_ATTRIBUTES.get(entity_type, frozenset())\n"
    )
    .unwrap();
    writeln!(
        out,
        "def get_entity_keys(entity_type: str) -> frozenset[str]:\n    \"\"\"Get key attribute names for an entity type.\"\"\"\n    return ENTITY_KEYS.get(entity_type, frozenset())\n"
    )
    .unwrap();
    writeln!(
        out,
        "def is_abstract_entity(entity_type: str) -> bool:\n    \"\"\"Check if an entity type is abstract.\"\"\"\n    return entity_type in ENTITY_ABSTRACT\n"
    )
    .unwrap();
    writeln!(
        out,
        "def is_abstract_relation(relation_type: str) -> bool:\n    \"\"\"Check if a relation type is abstract.\"\"\"\n    return relation_type in RELATION_ABSTRACT\n"
    )
    .unwrap();
    writeln!(out, "__all__ = [").unwrap();
    for name in [
        "SCHEMA_VERSION",
        "SCHEMA_HASH",
        "ENTITY_TYPES",
        "RELATION_TYPES",
        "ATTRIBUTE_TYPES",
        "EntityType",
        "RelationType",
        "AttributeType",
        "ENTITY_MAP",
        "RELATION_MAP",
        "ATTRIBUTE_MAP",
        "RoleInfo",
        "RELATION_ROLES",
        "ENTITY_ATTRIBUTES",
        "ENTITY_KEYS",
        "ATTRIBUTE_VALUE_TYPES",
        "ENTITY_PARENTS",
        "RELATION_PARENTS",
        "ENTITY_ABSTRACT",
        "RELATION_ABSTRACT",
        "ENTITY_ANNOTATIONS",
        "ATTRIBUTE_ANNOTATIONS",
        "RELATION_ANNOTATIONS",
        "ENTITY_TYPE_JSON_SCHEMA",
        "RELATION_TYPE_JSON_SCHEMA",
        "ATTRIBUTE_TYPE_JSON_SCHEMA",
        "get_entity_class",
        "get_relation_class",
        "get_attribute_class",
        "get_role_players",
        "get_entity_attributes",
        "get_entity_keys",
        "is_abstract_entity",
        "is_abstract_relation",
    ] {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "]").unwrap();
    out
}

fn write_py_tuple(out: &mut String, name: &str, values: &[String]) {
    writeln!(out, "{name}: tuple[str, ...] = (").unwrap();
    for value in values {
        writeln!(out, "    {},", string_literal(value)).unwrap();
    }
    writeln!(out, ")\n").unwrap();
}

fn write_py_enum(out: &mut String, enum_name: &str, kind: &str, values: &[String]) {
    writeln!(out, "class {enum_name}(StrEnum):").unwrap();
    writeln!(out, "    \"\"\"Enum of all {kind} type names.\"\"\"").unwrap();
    if values.is_empty() {
        writeln!(out, "    pass").unwrap();
    } else {
        for value in values {
            writeln!(
                out,
                "    {} = {}",
                value.replace('-', "_").to_uppercase(),
                string_literal(value)
            )
            .unwrap();
        }
    }
    writeln!(out).unwrap();
}

fn write_py_class_map(
    out: &mut String,
    name: &str,
    type_name: &str,
    module: &str,
    values: &[String],
) {
    writeln!(out, "{name}: dict[str, type[\"{type_name}\"]] = {{").unwrap();
    for value in values {
        writeln!(
            out,
            "    {}: {module}.{},",
            string_literal(value),
            class_name(value)
        )
        .unwrap();
    }
    writeln!(out, "}}\n").unwrap();
}

fn write_py_attr_sets<F>(out: &mut String, name: &str, values: &[String], getter: F)
where
    F: Fn(&str) -> Vec<String>,
{
    writeln!(out, "{name}: dict[str, frozenset[str]] = {{").unwrap();
    for value in values {
        let attrs = getter(value);
        if attrs.is_empty() {
            writeln!(out, "    {}: frozenset(),", string_literal(value)).unwrap();
        } else {
            writeln!(
                out,
                "    {}: frozenset({{{}}}),",
                string_literal(value),
                attrs
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(|attr| string_literal(attr))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
        }
    }
    writeln!(out, "}}\n").unwrap();
}

fn write_py_parent_map<F>(out: &mut String, name: &str, values: &[String], getter: F)
where
    F: Fn(&str) -> Option<String>,
{
    writeln!(out, "{name}: dict[str, str | None] = {{").unwrap();
    for value in values {
        match getter(value) {
            Some(parent) => writeln!(
                out,
                "    {}: {},",
                string_literal(value),
                string_literal(&parent)
            )
            .unwrap(),
            None => writeln!(out, "    {}: None,", string_literal(value)).unwrap(),
        }
    }
    writeln!(out, "}}\n").unwrap();
}

fn write_py_frozenset(out: &mut String, name: &str, values: &[String]) {
    writeln!(out, "{name}: frozenset[str] = frozenset({{").unwrap();
    for value in values {
        writeln!(out, "    {},", string_literal(value)).unwrap();
    }
    writeln!(out, "}})\n").unwrap();
}

fn write_py_annotations(
    out: &mut String,
    name: &str,
    annotations: &BTreeMap<String, AnnotationMap>,
) {
    writeln!(
        out,
        "{name}: dict[str, dict[str, bool | int | float | str | list]] = {{"
    )
    .unwrap();
    for (type_name, values) in annotations {
        let filtered = values
            .iter()
            .filter(|(key, _)| key.as_str() != "_docstring")
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            continue;
        }
        writeln!(out, "    {}: {{", string_literal(type_name)).unwrap();
        for (key, value) in filtered {
            writeln!(
                out,
                "        {}: {},",
                string_literal(key),
                py_annotation_literal(value)
            )
            .unwrap();
        }
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "}}\n").unwrap();
}

fn py_tuple_literal(values: &[String]) -> String {
    if values.is_empty() {
        return "()".to_string();
    }
    let entries = values
        .iter()
        .map(|value| string_literal(value))
        .collect::<Vec<_>>();
    if entries.len() == 1 {
        format!("({},)", entries[0])
    } else {
        format!("({})", entries.join(", "))
    }
}

fn render_python_package_init(
    schema: &TypeSchema,
    options: &BindgenOptions,
    functions_present: bool,
) -> String {
    let include_schema_loader = options.schema_filename.is_some();
    let mut module_imports = vec!["attributes", "entities", "registry", "relations"];
    if functions_present {
        module_imports.push("functions");
    }
    module_imports.sort();

    let mut all_exports = vec![
        "ATTRIBUTES",
        "ENTITIES",
        "RELATIONS",
        "SCHEMA_VERSION",
        "attributes",
        "entities",
        "registry",
        "relations",
    ];
    if functions_present {
        all_exports.push("functions");
    }
    if include_schema_loader {
        all_exports.push("schema_text");
    }
    all_exports.sort();

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"TypeBridge schema package generated from a TypeDB schema.\n\nAUTO-GENERATED FILE - DO NOT EDIT MANUALLY\nRegenerate with: type-bridge generate <schema.tql> <output_dir>\n\"\"\"\n"
    )
    .unwrap();
    writeln!(out, "from __future__ import annotations").unwrap();
    if include_schema_loader {
        writeln!(out).unwrap();
        writeln!(out, "from importlib import resources").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "from . import {}", module_imports.join(", ")).unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "SCHEMA_VERSION = {}",
        string_literal(&options.schema_version)
    )
    .unwrap();
    if let Some(filename) = options.schema_filename.as_deref() {
        writeln!(out).unwrap();
        writeln!(out).unwrap();
        writeln!(out, "def schema_text() -> str:").unwrap();
        writeln!(
            out,
            "    \"\"\"Return the canonical TypeDB schema text bundled with the package.\"\"\""
        )
        .unwrap();
        writeln!(out, "    return (").unwrap();
        writeln!(out, "        resources.files(__package__)").unwrap();
        writeln!(out, "        .joinpath({})", string_literal(filename)).unwrap();
        writeln!(out, "        .read_text(encoding=\"utf-8\")").unwrap();
        writeln!(out, "    )").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "ATTRIBUTES = [").unwrap();
    for name in schema
        .attributes
        .keys()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    attributes.{name},").unwrap();
    }
    writeln!(out, "]\n").unwrap();
    writeln!(out, "ENTITIES = [").unwrap();
    for name in schema
        .entities
        .keys()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    entities.{name},").unwrap();
    }
    writeln!(out, "]\n").unwrap();
    writeln!(out, "RELATIONS = [").unwrap();
    for name in schema
        .relations
        .keys()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    relations.{name},").unwrap();
    }
    writeln!(out, "]\n").unwrap();
    writeln!(out, "__all__ = [").unwrap();
    for name in all_exports {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "]").unwrap();
    out
}

fn python_type(value_type: &str) -> &'static str {
    match value_type {
        "string" => "str",
        "integer" | "int" | "long" => "int",
        "double" => "float",
        "boolean" | "bool" => "bool",
        "date" => "date",
        "datetime" | "datetime-tz" => "datetime",
        "decimal" => "Decimal",
        "duration" => "Duration",
        _ => "str",
    }
}

fn render_python_functions(schema: &TypeSchema, _options: &BindgenOptions) -> Option<String> {
    if schema.functions.is_empty() {
        return None;
    }

    let mut datetime_imports = BTreeSet::new();
    let mut has_decimal = false;
    let mut has_duration = false;
    for function in schema.functions.values() {
        for parameter in &function.parameters {
            match parameter.type_.trim_end_matches('?') {
                "date" => {
                    datetime_imports.insert("date");
                }
                "datetime" | "datetime-tz" => {
                    datetime_imports.insert("datetime");
                }
                "decimal" => has_decimal = true,
                "duration" => has_duration = true,
                _ => {}
            }
        }
        for item in &function.return_type.types {
            match item.name.as_str() {
                "date" => {
                    datetime_imports.insert("date");
                }
                "datetime" | "datetime-tz" => {
                    datetime_imports.insert("datetime");
                }
                "decimal" => has_decimal = true,
                "duration" => has_duration = true,
                _ => {}
            }
        }
    }

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"Function wrappers generated from a TypeDB schema.\n\nThese functions return FunctionQuery objects that can generate TypeQL queries\nfor calling the corresponding TypeDB schema functions.\n\"\"\"\n"
    )
    .unwrap();
    writeln!(out, "from __future__ import annotations\n").unwrap();
    if !datetime_imports.is_empty() {
        writeln!(
            out,
            "from datetime import {}",
            datetime_imports.into_iter().collect::<Vec<_>>().join(", ")
        )
        .unwrap();
    }
    if has_decimal {
        writeln!(out, "from decimal import Decimal").unwrap();
    }
    if has_duration {
        writeln!(out, "from isodate import Duration").unwrap();
    }
    writeln!(out, "from typing import Iterator\n").unwrap();
    writeln!(
        out,
        "from type_bridge.expressions import FunctionQuery, ReturnType\n"
    )
    .unwrap();
    writeln!(out).unwrap();

    let mut exports = Vec::new();
    for function in schema.functions.values() {
        let py_name = field_name(&function.name);
        exports.push(py_name.clone());
        let params = function
            .parameters
            .iter()
            .map(|parameter| {
                let name = field_name(&parameter.name);
                format!("{}: {} | str", name, python_type(&parameter.type_))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_types = function
            .return_type
            .types
            .iter()
            .map(|item| {
                let suffix = if item.optional { "?" } else { "" };
                format!("{}{}", item.name, suffix)
            })
            .collect::<Vec<_>>();
        let inner_types = return_types
            .iter()
            .map(|type_name| python_type(type_name.trim_end_matches('?')).to_string())
            .collect::<Vec<_>>();
        let inner = if inner_types.len() == 1 {
            inner_types[0].clone()
        } else {
            format!("tuple[{}]", inner_types.join(", "))
        };
        let return_hint = if function.return_type.is_stream {
            format!("FunctionQuery[Iterator[{inner}]]")
        } else {
            format!("FunctionQuery[{inner}]")
        };
        if params.is_empty() {
            writeln!(out, "def {py_name}() -> {return_hint}:").unwrap();
        } else {
            writeln!(out, "def {py_name}({params}) -> {return_hint}:").unwrap();
        }
        writeln!(out, "    \"\"\"Call TypeDB function `{}`.\n", function.name).unwrap();
        let stream = if function.return_type.is_stream {
            "stream of "
        } else {
            ""
        };
        writeln!(out, "    Returns: {stream}{}", return_types.join(", ")).unwrap();
        writeln!(out, "    \"\"\"").unwrap();
        let args = if function.parameters.is_empty() {
            "[]".to_string()
        } else {
            let args = function
                .parameters
                .iter()
                .map(|parameter| {
                    format!("(\"${}\", {})", parameter.name, field_name(&parameter.name))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{args}]")
        };
        writeln!(out, "    return FunctionQuery(").unwrap();
        writeln!(out, "        name={},", string_literal(&function.name)).unwrap();
        writeln!(out, "        args={args},").unwrap();
        writeln!(
            out,
            "        return_type=ReturnType([{}], is_stream={}),",
            return_types
                .iter()
                .map(|type_name| string_literal(type_name))
                .collect::<Vec<_>>()
                .join(", "),
            bool_py(function.return_type.is_stream)
        )
        .unwrap();
        writeln!(out, "    )\n").unwrap();
    }
    writeln!(out, "__all__ = [").unwrap();
    for name in exports.into_iter().collect::<BTreeSet<_>>() {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "]").unwrap();
    Some(out)
}

fn render_python_structs(schema: &TypeSchema, options: &BindgenOptions) -> Option<String> {
    if schema.structs.is_empty() {
        return None;
    }
    let mut datetime_imports = BTreeSet::new();
    let mut decimal_import = false;
    for structure in schema.structs.values() {
        for field in &structure.fields {
            match field.value_type.as_str() {
                "date" => {
                    datetime_imports.insert("date");
                }
                "datetime" | "datetime-tz" => {
                    datetime_imports.insert("datetime");
                }
                "decimal" => decimal_import = true,
                _ => {}
            }
        }
    }

    let mut out = String::new();
    writeln!(
        out,
        "\"\"\"Struct type definitions generated from a TypeDB schema.\n\nStructs are composite value types introduced in TypeDB 3.0.\nThey are rendered as frozen dataclasses for immutability.\n\nAUTO-GENERATED FILE - DO NOT EDIT MANUALLY\nRegenerate with: type-bridge generate <schema.tql> <output_dir>\n\"\"\"\n"
    )
    .unwrap();
    writeln!(out, "from __future__ import annotations\n").unwrap();
    writeln!(out, "from dataclasses import dataclass").unwrap();
    if !datetime_imports.is_empty() {
        writeln!(
            out,
            "from datetime import {}",
            datetime_imports.into_iter().collect::<Vec<_>>().join(", ")
        )
        .unwrap();
    }
    if decimal_import {
        writeln!(out, "from decimal import Decimal").unwrap();
    }
    writeln!(out).unwrap();

    for structure in schema.structs.values() {
        let class = class_name(&structure.name);
        let doc = docstring(
            &options.python_metadata.attribute_annotations,
            &structure.name,
        )
        .unwrap_or_else(|| format!("Struct for `{}`.", structure.name));
        writeln!(out).unwrap();
        writeln!(out, "@dataclass(frozen=True, slots=True)").unwrap();
        writeln!(out, "class {class}:").unwrap();
        writeln!(out, "    \"\"\"{doc}\"\"\"").unwrap();
        if structure.fields.is_empty() {
            writeln!(out, "    pass").unwrap();
        }
        for field in &structure.fields {
            let mut ty = python_type(&field.value_type).to_string();
            if field.optional {
                ty.push_str(" | None");
                writeln!(out, "    {}: {ty} = None", field_name(&field.name)).unwrap();
            } else {
                writeln!(out, "    {}: {ty}", field_name(&field.name)).unwrap();
            }
        }
        writeln!(out).unwrap();
    }
    writeln!(out, "__all__ = [").unwrap();
    for name in schema
        .structs
        .keys()
        .map(|name| class_name(name))
        .collect::<BTreeSet<_>>()
    {
        writeln!(out, "    \"{name}\",").unwrap();
    }
    writeln!(out, "]").unwrap();
    Some(out)
}

// ---------------------------------------------------------------------------
// TypeScript renderer
// ---------------------------------------------------------------------------

fn render_typescript_package(schema: &TypeSchema, options: &BindgenOptions) -> GeneratedPackage {
    GeneratedPackage {
        target: TargetLanguage::TypeScript,
        files: vec![
            GeneratedFile {
                path: "attributes.ts".to_string(),
                contents: render_ts_attributes(schema),
            },
            GeneratedFile {
                path: "entities.ts".to_string(),
                contents: render_ts_entities(schema, options),
            },
            GeneratedFile {
                path: "relations.ts".to_string(),
                contents: render_ts_relations(schema, options),
            },
            GeneratedFile {
                path: "index.ts".to_string(),
                contents: "export * from \"./attributes.js\";\nexport * from \"./entities.js\";\nexport * from \"./relations.js\";\n".to_string(),
            },
        ],
    }
}

fn render_ts_attributes(schema: &TypeSchema) -> String {
    let order = topological_sort(&schema.attributes, |attr| attr.parent.as_deref());
    let mut out = String::new();
    writeln!(out, "import {{ attr }} from \"@type-bridge/node\";\n").unwrap();
    for name in order {
        let attr = &schema.attributes[&name];
        let mut options = Vec::new();
        if let Some(parent) = attr.parent.as_deref() {
            if schema.attributes.contains_key(parent) {
                options.push(format!("parent: {}", class_name(parent)));
            } else {
                options.push(format!("parent: {}", string_literal(parent)));
            }
        }
        if attr.is_abstract {
            options.push("abstract: true".to_string());
        }
        if attr.is_independent {
            options.push("independent: true".to_string());
        }
        if let Some(regex) = attr.regex.as_deref() {
            options.push(format!("regex: {}", string_literal(regex)));
        }
        if let Some(values) = attr.allowed_values.as_ref() {
            options.push(format!(
                "values: [{}]",
                values
                    .iter()
                    .map(|value| string_literal(value))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if attr.range_min.is_some() || attr.range_max.is_some() {
            options.push(format!(
                "range: [{}, {}]",
                attr.range_min
                    .as_deref()
                    .map(string_literal)
                    .unwrap_or_else(|| "null".to_string()),
                attr.range_max
                    .as_deref()
                    .map(string_literal)
                    .unwrap_or_else(|| "null".to_string())
            ));
        }
        if let Some(doc_text) = attr.doc.as_deref() {
            options.push(format!("doc: {}", string_literal(doc_text)));
        }
        if !attr.meta.is_empty() {
            options.push(format!("meta: {}", ts_meta_literal(&attr.meta)));
        }
        let options = if options.is_empty() {
            String::new()
        } else {
            format!(", {{ {} }}", options.join(", "))
        };
        writeln!(
            out,
            "export class {} extends attr.{}({}{options}) {{}}",
            class_name(&name),
            ts_attr_kind(resolved_attr_value_type(schema, &name)),
            string_literal(&name)
        )
        .unwrap();
    }
    out
}

fn ts_field_expr(owned: &OwnedAttribute, attr_class: &str, is_key: bool) -> String {
    let mut extras = doc_meta_flag_args(owned.doc.as_deref(), &owned.meta);
    if is_key {
        return format!("field({attr_class}, Key{extras})");
    }
    // @unique does not imply required: unlike @key it keeps the default
    // card(0..1), so it composes with the cardinality handling below as a
    // plain marker instead of short-circuiting the optionality logic.
    if owned.is_unique {
        extras = format!(", Unique{extras}");
    }
    if owned.ordered {
        let base = format!("field({attr_class}{extras}).ordered()");
        return if owned.distinct {
            format!("{base}.distinct()")
        } else {
            base
        };
    }
    match owned.cardinality.as_ref() {
        None => format!("field({attr_class}{extras}).optional()"),
        Some(cardinality) if card_is_optional_single(cardinality) => {
            format!("field({attr_class}{extras}).optional()")
        }
        Some(cardinality) if card_is_multi(cardinality) => {
            format!(
                "field({attr_class}{extras}).list({})",
                ts_card_expr(cardinality)
            )
        }
        Some(cardinality) if card_is_required_single(cardinality) => {
            format!("field({attr_class}{extras})")
        }
        _ => format!("field({attr_class}{extras}).optional()"),
    }
}

fn render_ts_entities(schema: &TypeSchema, options: &BindgenOptions) -> String {
    let order = topological_sort(&schema.entities, |entity| entity.parent.as_deref());
    let implicit_keys = options
        .implicit_key_attributes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut referenced_attrs = BTreeSet::new();
    let mut factory_imports = BTreeSet::from(["Entity", "field"]);

    for name in &order {
        let entity = &schema.entities[name];
        let parent_owns = direct_parent_owns(schema, entity.parent.as_ref(), false);
        if entity.is_abstract || entity.doc.is_some() || !entity.meta.is_empty() {
            factory_imports.insert("TypeFlags");
        }
        for owned in &entity.owns {
            if parent_owns.contains(owned.name.as_str()) {
                continue;
            }
            referenced_attrs.insert(class_name(&owned.name));
            if owned.is_key || implicit_keys.contains(owned.name.as_str()) {
                factory_imports.insert("Key");
            }
            if owned.is_unique {
                factory_imports.insert("Unique");
            }
            if owned.cardinality.as_ref().is_some_and(card_is_multi) {
                factory_imports.insert("Card");
            }
            if owned.ordered {
                factory_imports.insert("Ordered");
            }
            if owned.distinct {
                factory_imports.insert("Distinct");
            }
            if owned.doc.is_some() {
                factory_imports.insert("Doc");
            }
            if !owned.meta.is_empty() {
                factory_imports.insert("Meta");
            }
        }
    }

    let mut out = String::new();
    writeln!(
        out,
        "import {{ {} }} from \"@type-bridge/node\";",
        factory_imports.into_iter().collect::<Vec<_>>().join(", ")
    )
    .unwrap();
    if !referenced_attrs.is_empty() {
        writeln!(
            out,
            "import {{ {} }} from \"./attributes.js\";",
            referenced_attrs.into_iter().collect::<Vec<_>>().join(", ")
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    for name in &order {
        let entity = &schema.entities[name];
        let class = class_name(name);
        let first_arg = ts_type_first_arg(
            name,
            entity.is_abstract,
            entity.doc.as_deref(),
            &entity.meta,
        );
        let third_arg = entity
            .parent
            .as_deref()
            .filter(|parent| schema.entities.contains_key(*parent))
            .map(|parent| format!(", {{ parent: {} }}", class_name(parent)))
            .unwrap_or_default();

        let parent_owns = direct_parent_owns(schema, entity.parent.as_ref(), false);
        let mut fields = Vec::new();
        for owned in ordered_owned_attributes(&entity.owns, &entity.owns_order) {
            if parent_owns.contains(owned.name.as_str())
                || !schema.attributes.contains_key(&owned.name)
            {
                continue;
            }
            let is_key = owned.is_key || implicit_keys.contains(owned.name.as_str());
            fields.push(format!(
                "  {}: {},",
                field_name(&owned.name),
                ts_field_expr(owned, &class_name(&owned.name), is_key)
            ));
        }
        if fields.is_empty() {
            writeln!(
                out,
                "export class {class} extends Entity({first_arg}, {{}}{third_arg}) {{}}"
            )
            .unwrap();
        } else {
            writeln!(out, "export class {class} extends Entity({first_arg}, {{").unwrap();
            for field in fields {
                writeln!(out, "{field}").unwrap();
            }
            writeln!(out, "}}{third_arg}) {{}}").unwrap();
        }
        writeln!(out).unwrap();
    }
    out
}

/// Render the first `Entity(...)`/`Relation(...)` argument: a plain name
/// literal, or a `TypeFlags({...})` call when the type carries flags or
/// doc/meta annotations.
fn ts_type_first_arg(
    name: &str,
    is_abstract: bool,
    doc: Option<&str>,
    meta: &BTreeMap<String, String>,
) -> String {
    if !is_abstract && doc.is_none() && meta.is_empty() {
        return string_literal(name);
    }
    let mut opts = vec![format!("name: {}", string_literal(name))];
    if is_abstract {
        opts.push("abstract: true".to_string());
    }
    if let Some(doc) = doc {
        opts.push(format!("doc: {}", string_literal(doc)));
    }
    if !meta.is_empty() {
        opts.push(format!("meta: {}", ts_meta_literal(meta)));
    }
    format!("TypeFlags({{ {} }})", opts.join(", "))
}

fn ts_role_call(
    player_classes: &[String],
    role: &RoleSpec,
    plays_cardinality: Option<&Cardinality>,
) -> String {
    let mut options = Vec::new();
    if let Some(cardinality) = role.cardinality.as_ref()
        && !(cardinality.min == 1 && cardinality.max == Some(1))
    {
        options.push(format!("cardinality: {}", ts_card_expr(cardinality)));
    }
    if let Some(cardinality) = plays_cardinality {
        options.push(format!("playsCardinality: {}", ts_card_expr(cardinality)));
    }
    if let Some(overrides) = role.overrides.as_deref() {
        options.push(format!("overrides: {}", string_literal(overrides)));
    }
    if role.is_abstract {
        options.push("abstract: true".to_string());
    }
    if role.ordered {
        options.push("ordered: true".to_string());
    }
    if role.distinct {
        options.push("distinct: true".to_string());
    }
    if let Some(doc_text) = role.doc.as_deref() {
        options.push(format!("doc: {}", string_literal(doc_text)));
    }
    if !role.meta.is_empty() {
        options.push(format!("meta: {}", ts_meta_literal(&role.meta)));
    }
    let option_arg = if options.is_empty() {
        String::new()
    } else {
        format!(", {{ {} }}", options.join(", "))
    };
    format!("role({}{option_arg})", player_classes.join(", "))
}

fn render_ts_relations(schema: &TypeSchema, options: &BindgenOptions) -> String {
    let order = topological_sort(&schema.relations, |relation| relation.parent.as_deref());
    let implicit_keys = options
        .implicit_key_attributes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut referenced_attrs = BTreeSet::new();
    let mut referenced_entities = BTreeSet::new();
    let mut factory_imports = BTreeSet::from(["Relation", "field", "role"]);

    for name in &order {
        let relation = &schema.relations[name];
        if relation.is_abstract || relation.doc.is_some() || !relation.meta.is_empty() {
            factory_imports.insert("TypeFlags");
        }
        let parent_owns = direct_parent_owns(schema, relation.parent.as_ref(), true);
        for owned in &relation.owns {
            if parent_owns.contains(owned.name.as_str()) {
                continue;
            }
            referenced_attrs.insert(class_name(&owned.name));
            if owned.is_key || implicit_keys.contains(owned.name.as_str()) {
                factory_imports.insert("Key");
            }
            if owned.is_unique {
                factory_imports.insert("Unique");
            }
            if owned.cardinality.as_ref().is_some_and(card_is_multi) {
                factory_imports.insert("Card");
            }
            if owned.ordered {
                factory_imports.insert("Ordered");
            }
            if owned.distinct {
                factory_imports.insert("Distinct");
            }
            if owned.doc.is_some() {
                factory_imports.insert("Doc");
            }
            if !owned.meta.is_empty() {
                factory_imports.insert("Meta");
            }
        }
        let parent_roles = direct_parent_roles(schema, relation.parent.as_ref());
        for role in &relation.roles {
            if parent_roles.contains(role.name.as_str()) && role.overrides.is_none() {
                continue;
            }
            if role
                .cardinality
                .as_ref()
                .is_some_and(|cardinality| !(cardinality.min == 1 && cardinality.max == Some(1)))
            {
                factory_imports.insert("Card");
            }
            if role.ordered {
                factory_imports.insert("Ordered");
            }
            if role.distinct {
                factory_imports.insert("Distinct");
            }
            let players = minimal_role_players(schema, name, &role.name);
            if plays_cardinality_for_role(schema, name, &role.name, &players).is_some() {
                factory_imports.insert("Card");
            }
            for player in players {
                referenced_entities.insert(class_name(&player));
            }
        }
    }

    let mut out = String::new();
    writeln!(
        out,
        "import {{ {} }} from \"@type-bridge/node\";",
        factory_imports.into_iter().collect::<Vec<_>>().join(", ")
    )
    .unwrap();
    if !referenced_attrs.is_empty() {
        writeln!(
            out,
            "import {{ {} }} from \"./attributes.js\";",
            referenced_attrs.into_iter().collect::<Vec<_>>().join(", ")
        )
        .unwrap();
    }
    if !referenced_entities.is_empty() {
        writeln!(
            out,
            "import {{ {} }} from \"./entities.js\";",
            referenced_entities
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    for name in &order {
        let relation = &schema.relations[name];
        let class = class_name(name);
        let first_arg = ts_type_first_arg(
            name,
            relation.is_abstract,
            relation.doc.as_deref(),
            &relation.meta,
        );
        let third_arg = relation
            .parent
            .as_deref()
            .filter(|parent| schema.relations.contains_key(*parent))
            .map(|parent| format!(", {{ parent: {} }}", class_name(parent)))
            .unwrap_or_default();

        let mut fields = Vec::new();
        let parent_roles = direct_parent_roles(schema, relation.parent.as_ref());
        for role in &relation.roles {
            if parent_roles.contains(role.name.as_str()) && role.overrides.is_none() {
                continue;
            }
            let players = minimal_role_players(schema, name, &role.name);
            if players.is_empty() {
                continue;
            }
            let player_classes = players
                .iter()
                .map(|player| class_name(player))
                .collect::<Vec<_>>();
            let plays_cardinality = plays_cardinality_for_role(schema, name, &role.name, &players);
            fields.push(format!(
                "  {}: {},",
                field_name(&role.name),
                ts_role_call(&player_classes, role, plays_cardinality.as_ref())
            ));
        }

        let parent_owns = direct_parent_owns(schema, relation.parent.as_ref(), true);
        for owned in ordered_owned_attributes(&relation.owns, &relation.owns_order) {
            if parent_owns.contains(owned.name.as_str())
                || !schema.attributes.contains_key(&owned.name)
            {
                continue;
            }
            let is_key = owned.is_key || implicit_keys.contains(owned.name.as_str());
            fields.push(format!(
                "  {}: {},",
                field_name(&owned.name),
                ts_field_expr(owned, &class_name(&owned.name), is_key)
            ));
        }

        if fields.is_empty() {
            writeln!(
                out,
                "export class {class} extends Relation({first_arg}, {{}}{third_arg}) {{}}"
            )
            .unwrap();
        } else {
            writeln!(out, "export class {class} extends Relation({first_arg}, {{").unwrap();
            for field in fields {
                writeln!(out, "{field}").unwrap();
            }
            writeln!(out, "}}{third_arg}) {{}}").unwrap();
        }
        writeln!(out).unwrap();
    }
    out
}

// ---------------------------------------------------------------------------
// Rust renderer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum RustRenderMode {
    Module,
    Inline,
}

fn render_rust_models(schema: &TypeSchema, mode: RustRenderMode) -> GeneratedRustModels {
    GeneratedRustModels {
        mod_rs: match mode {
            RustRenderMode::Module => render_rust_mod_rs(),
            RustRenderMode::Inline => String::new(),
        },
        attributes_rs: render_rust_attributes(schema, mode),
        entities_rs: render_rust_entities(schema, mode),
        relations_rs: render_rust_relations(schema, mode),
    }
}

fn render_rust_mod_rs() -> String {
    "//! Generated models.\n//!\n//! Auto-generated by type-bridge codegen. Do not edit manually.\n\npub mod attributes;\npub mod entities;\npub mod relations;\n".to_string()
}

fn rust_header(out: &mut String, title: &str) {
    writeln!(out, "//! Generated {title} types.").unwrap();
    writeln!(out, "//!").unwrap();
    writeln!(
        out,
        "//! Auto-generated by type-bridge codegen. Do not edit manually."
    )
    .unwrap();
    writeln!(out).unwrap();
}

fn render_rust_attributes(schema: &TypeSchema, mode: RustRenderMode) -> String {
    let mut out = String::new();
    if matches!(mode, RustRenderMode::Module) {
        rust_header(&mut out, "attribute");
        writeln!(out, "use type_bridge_orm::define_attribute;\n").unwrap();
    }
    for attr in schema.attributes.values() {
        if attr.is_abstract {
            continue;
        }
        let prefix = if matches!(mode, RustRenderMode::Inline) {
            "type_bridge_orm::"
        } else {
            ""
        };
        writeln!(
            out,
            "{prefix}define_attribute!({}, {}, {});",
            rust_pascal_case(&attr.name),
            string_literal(&attr.name),
            string_literal(rust_value_type(resolved_attr_value_type(
                schema, &attr.name
            )))
        )
        .unwrap();
    }
    out
}

fn render_rust_entities(schema: &TypeSchema, mode: RustRenderMode) -> String {
    let mut out = String::new();
    if matches!(mode, RustRenderMode::Module) {
        rust_header(&mut out, "entity");
        writeln!(out, "use type_bridge_orm::{{DeriveEntity, Result}};").unwrap();
        writeln!(out, "use super::attributes::*;\n").unwrap();
    }
    let order = topological_sort(&schema.entities, |entity| entity.parent.as_deref());
    for name in order {
        render_rust_entity(&mut out, schema, &schema.entities[&name], mode);
        writeln!(out).unwrap();
    }
    out
}

fn render_rust_entity(
    out: &mut String,
    schema: &TypeSchema,
    entity: &EntityType,
    mode: RustRenderMode,
) {
    let derive = if matches!(mode, RustRenderMode::Inline) {
        "type_bridge_orm::DeriveEntity"
    } else {
        "DeriveEntity"
    };
    let mut parts = vec![format!("name = {}", string_literal(&entity.name))];
    if entity.is_abstract {
        parts.push("r#abstract".to_string());
    }
    if let Some(parent) = entity.parent.as_deref() {
        parts.push(format!("extends = {}", string_literal(parent)));
    }
    push_rust_doc_meta_parts(&mut parts, entity.doc.as_deref(), &entity.meta);
    rust_doc_comment(out, entity.doc.as_deref(), "");
    writeln!(out, "#[derive({derive}, Debug)]").unwrap();
    writeln!(out, "#[entity({})]", parts.join(", ")).unwrap();
    writeln!(out, "pub struct {} {{", rust_pascal_case(&entity.name)).unwrap();
    writeln!(out, "    pub iid: Option<String>,").unwrap();
    for owned in ordered_owned_attributes(&entity.owns, &entity.owns_order) {
        let attr_type = rust_pascal_case(&owned.name);
        rust_doc_comment(out, owned.doc.as_deref(), "    ");
        if let Some(field_attr) = rust_field_attribute(owned) {
            writeln!(out, "    {field_attr}").unwrap();
        }
        writeln!(
            out,
            "    pub {}: {},",
            field_name(&owned.name),
            rust_field_type(&attr_type, owned)
        )
        .unwrap();
    }
    let _ = schema;
    writeln!(out, "}}").unwrap();
}

fn render_rust_relations(schema: &TypeSchema, mode: RustRenderMode) -> String {
    let mut out = String::new();
    if matches!(mode, RustRenderMode::Module) {
        rust_header(&mut out, "relation");
        writeln!(
            out,
            "use type_bridge_orm::{{DeriveRelation, RolePlayerRef, Result}};"
        )
        .unwrap();
        writeln!(out, "use super::attributes::*;\n").unwrap();
    }
    let order = topological_sort(&schema.relations, |relation| relation.parent.as_deref());
    for name in order {
        render_rust_relation(&mut out, schema, &schema.relations[&name], mode);
        writeln!(out).unwrap();
    }
    out
}

fn render_rust_relation(
    out: &mut String,
    schema: &TypeSchema,
    relation: &RelationType,
    mode: RustRenderMode,
) {
    let derive = if matches!(mode, RustRenderMode::Inline) {
        "type_bridge_orm::DeriveRelation"
    } else {
        "DeriveRelation"
    };
    let role_type = if matches!(mode, RustRenderMode::Inline) {
        "type_bridge_orm::RolePlayerRef"
    } else {
        "RolePlayerRef"
    };
    let mut parts = vec![format!("name = {}", string_literal(&relation.name))];
    if relation.is_abstract {
        parts.push("r#abstract".to_string());
    }
    if let Some(parent) = relation.parent.as_deref() {
        parts.push(format!("extends = {}", string_literal(parent)));
    }
    push_rust_doc_meta_parts(&mut parts, relation.doc.as_deref(), &relation.meta);
    rust_doc_comment(out, relation.doc.as_deref(), "");
    writeln!(out, "#[derive({derive}, Debug)]").unwrap();
    writeln!(out, "#[relation({})]", parts.join(", ")).unwrap();
    writeln!(out, "pub struct {} {{", rust_pascal_case(&relation.name)).unwrap();
    writeln!(out, "    pub iid: Option<String>,").unwrap();

    let mut seen = HashSet::new();
    for role in &relation.roles {
        let player_type = resolve_rust_role_player(schema, &relation.name, &role.name);
        let role_field = unique_role_field(&role.name, &mut seen);
        let mut role_parts = vec![
            format!("name = {}", string_literal(&role.name)),
            format!("player_type = {}", string_literal(&player_type)),
        ];
        push_rust_doc_meta_parts(&mut role_parts, role.doc.as_deref(), &role.meta);
        rust_doc_comment(out, role.doc.as_deref(), "    ");
        writeln!(out, "    #[role({})]", role_parts.join(", ")).unwrap();
        writeln!(out, "    pub {role_field}: {role_type},").unwrap();
    }

    for owned in ordered_owned_attributes(&relation.owns, &relation.owns_order) {
        let attr_type = rust_pascal_case(&owned.name);
        rust_doc_comment(out, owned.doc.as_deref(), "    ");
        if let Some(field_attr) = rust_field_attribute(owned) {
            writeln!(out, "    {field_attr}").unwrap();
        }
        writeln!(
            out,
            "    pub {}: Option<{}>,",
            field_name(&owned.name),
            rust_field_type(&attr_type, owned)
        )
        .unwrap();
    }
    writeln!(out, "}}").unwrap();
}

fn rust_field_attribute(owned: &OwnedAttribute) -> Option<String> {
    let mut parts = Vec::new();
    if owned.is_key {
        parts.push("key".to_string());
    }
    if owned.is_unique {
        parts.push("unique".to_string());
    }
    if let Some(cardinality) = owned.cardinality.as_ref() {
        parts.push(format!("card_min = {}", cardinality.min));
        if let Some(max) = cardinality.max {
            parts.push(format!("card_max = {max}"));
        }
    }
    push_rust_doc_meta_parts(&mut parts, owned.doc.as_deref(), &owned.meta);
    (!parts.is_empty()).then(|| format!("#[field({})]", parts.join(", ")))
}

/// Append `doc = "..."` and repeatable `meta("key", "value")` derive-attribute
/// parts, matching the `#[entity]`/`#[relation]`/`#[role]`/`#[field]` grammar.
fn push_rust_doc_meta_parts(
    parts: &mut Vec<String>,
    doc: Option<&str>,
    meta: &BTreeMap<String, String>,
) {
    if let Some(doc) = doc {
        parts.push(format!("doc = {}", string_literal(doc)));
    }
    for (key, value) in meta {
        parts.push(format!(
            "meta({}, {})",
            string_literal(key),
            string_literal(value)
        ));
    }
}

/// Write `/// ...` doc-comment lines for a `@doc` annotation, one per line of
/// the annotation text, at the given indentation.
fn rust_doc_comment(out: &mut String, doc: Option<&str>, indent: &str) {
    if let Some(doc) = doc {
        for line in doc.lines() {
            if line.is_empty() {
                writeln!(out, "{indent}///").unwrap();
            } else {
                writeln!(out, "{indent}/// {line}").unwrap();
            }
        }
    }
}

fn rust_field_type(attr_type: &str, owned: &OwnedAttribute) -> String {
    match owned.cardinality.as_ref() {
        Some(cardinality) if cardinality.min == 0 && cardinality.max == Some(1) => {
            format!("Option<{attr_type}>")
        }
        Some(cardinality) if cardinality.max.is_none_or(|max| max > 1) => {
            format!("Vec<{attr_type}>")
        }
        _ => attr_type.to_string(),
    }
}

fn resolve_rust_role_player(schema: &TypeSchema, relation_name: &str, role_name: &str) -> String {
    let role_ref = format!("{relation_name}:{role_name}");
    for entity in schema.entities.values() {
        if entity
            .plays
            .iter()
            .any(|played| played.role_ref == role_ref)
        {
            return entity.name.clone();
        }
    }
    for relation in schema.relations.values() {
        if relation
            .plays
            .iter()
            .any(|played| played.role_ref == role_ref)
        {
            return relation.name.clone();
        }
    }
    "unknown".to_string()
}

fn unique_role_field(role_name: &str, seen: &mut HashSet<String>) -> String {
    let base = field_name(role_name);
    if seen.insert(base.clone()) {
        return base;
    }
    for i in 1.. {
        let candidate = format!("{base}_{i}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_text() -> &'static str {
        "define
attribute name, value string;
attribute tag, value string;
entity party @abstract, owns name @key;
entity person sub party, owns tag @card(0..5), plays friendship:friend @card(0..2);
relation friendship, relates friend @card(1..2);"
    }

    #[test]
    fn renders_all_targets() {
        let plan = BindgenPlan::from_typeql(schema_text()).unwrap();
        for target in [
            TargetLanguage::Python,
            TargetLanguage::TypeScript,
            TargetLanguage::Rust,
        ] {
            let package = plan.render(target, &BindgenOptions::default());
            assert!(!package.files.is_empty());
        }
    }

    #[test]
    fn python_renders_expected_files_and_order() {
        let plan = BindgenPlan::from_typeql(schema_text()).unwrap();
        let package = plan.render(TargetLanguage::Python, &BindgenOptions::default());
        assert!(package.file("attributes.py").is_some());
        let entities = &package.file("entities.py").unwrap().contents;
        assert!(entities.find("class Party").unwrap() < entities.find("class Person").unwrap());
    }

    #[test]
    fn typescript_renders_cardinality_and_plays_cardinality() {
        let plan = BindgenPlan::from_typeql(schema_text()).unwrap();
        let package = plan.render(TargetLanguage::TypeScript, &BindgenOptions::default());
        let entities = &package.file("entities.ts").unwrap().contents;
        let relations = &package.file("relations.ts").unwrap().contents;
        assert!(entities.contains("tag: field(Tag).list(Card(0, 5))"));
        assert!(relations.contains(
            "friend: role(Person, { cardinality: Card(1, 2), playsCardinality: Card(0, 2) })"
        ));
    }

    #[test]
    fn rust_renders_existing_file_set() {
        let plan = BindgenPlan::from_typeql(schema_text()).unwrap();
        let models = plan.render_rust_models();
        assert!(models.mod_rs.contains("pub mod attributes;"));
        assert!(models.attributes_rs.contains("define_attribute!(Name"));
        assert!(
            models
                .entities_rs
                .contains("#[entity(name = \"party\", r#abstract)]")
        );
        assert!(models.relations_rs.contains("player_type = \"person\""));
    }

    #[test]
    fn doc_meta_annotations_render_on_all_targets() {
        let schema_text = r#"define
attribute name @doc("Name docs.") @meta("owner", "core"), value string;
attribute nick, value string;
entity party @abstract @doc("Party docs.") @meta("steward", "team"),
    owns name @key @doc("Ownership docs.") @meta("column", "name"),
    owns nick @card(0..1) @doc("Nick docs.");
entity person sub party, plays friendship:friend;
relation friendship @doc("Friendship docs."),
    relates friend @card(1..2) @doc("Role docs.") @meta("side", "a");"#;
        let plan = BindgenPlan::from_typeql(schema_text).unwrap();

        let python = plan.render(TargetLanguage::Python, &BindgenOptions::default());
        let attributes = &python.file("attributes.py").unwrap().contents;
        assert!(attributes.contains("\"\"\"Name docs.\"\"\""));
        assert!(attributes.contains("doc=\"Name docs.\", meta={\"owner\": \"core\"})"));
        let entities = &python.file("entities.py").unwrap().contents;
        assert!(entities.contains("\"\"\"Party docs.\"\"\""));
        assert!(
            entities.contains("abstract=True, doc=\"Party docs.\", meta={\"steward\": \"team\"}")
        );
        assert!(entities.contains(
            "name: attributes.Name = Flag(Key, Doc(\"Ownership docs.\"), Meta(\"column\", \"name\"))"
        ));
        assert!(entities.contains("nick: attributes.Nick | None = Flag(Doc(\"Nick docs.\"))"));
        let relations = &python.file("relations.py").unwrap().contents;
        assert!(relations.contains("\"\"\"Friendship docs.\"\"\""));
        assert!(relations.contains("doc=\"Friendship docs.\""));
        assert!(relations.contains("doc=\"Role docs.\", meta={\"side\": \"a\"}"));

        let typescript = plan.render(TargetLanguage::TypeScript, &BindgenOptions::default());
        let ts_attributes = &typescript.file("attributes.ts").unwrap().contents;
        assert!(ts_attributes.contains("doc: \"Name docs.\", meta: { \"owner\": \"core\" }"));
        let ts_entities = &typescript.file("entities.ts").unwrap().contents;
        assert!(ts_entities.contains(
            "TypeFlags({ name: \"party\", abstract: true, doc: \"Party docs.\", meta: { \"steward\": \"team\" } })"
        ));
        assert!(ts_entities.contains(
            "name: field(Name, Key, Doc(\"Ownership docs.\"), Meta(\"column\", \"name\"))"
        ));
        assert!(ts_entities.contains("nick: field(Nick, Doc(\"Nick docs.\")).optional()"));
        let ts_relations = &typescript.file("relations.ts").unwrap().contents;
        assert!(
            ts_relations.contains("TypeFlags({ name: \"friendship\", doc: \"Friendship docs.\" })")
        );
        assert!(ts_relations.contains("doc: \"Role docs.\", meta: { \"side\": \"a\" }"));

        let models = plan.render_rust_models();
        assert!(models.entities_rs.contains("/// Party docs."));
        assert!(models.entities_rs.contains(
            "#[entity(name = \"party\", r#abstract, doc = \"Party docs.\", meta(\"steward\", \"team\"))]"
        ));
        assert!(
            models
                .entities_rs
                .contains("#[field(key, doc = \"Ownership docs.\", meta(\"column\", \"name\"))]")
        );
        assert!(
            models
                .relations_rs
                .contains("#[relation(name = \"friendship\", doc = \"Friendship docs.\")]")
        );
        assert!(
            models
                .relations_rs
                .contains("doc = \"Role docs.\", meta(\"side\", \"a\"))]")
        );
    }

    #[test]
    fn unique_ownership_respects_cardinality() {
        let schema_text = r#"define
attribute email, value string;
attribute handle, value string;
attribute alias, value string;
entity person,
    owns email @unique,
    owns handle @unique @card(1..1),
    owns alias @unique @card(0..3);"#;
        let plan = BindgenPlan::from_typeql(schema_text).unwrap();

        let python = plan.render(TargetLanguage::Python, &BindgenOptions::default());
        let entities = &python.file("entities.py").unwrap().contents;
        assert!(entities.contains("email: attributes.Email | None = Flag(Unique)"));
        assert!(entities.contains("handle: attributes.Handle = Flag(Unique)"));
        assert!(entities.contains("alias: list[attributes.Alias] = Flag(Card(0, 3), Unique)"));

        let typescript = plan.render(TargetLanguage::TypeScript, &BindgenOptions::default());
        let ts_entities = &typescript.file("entities.ts").unwrap().contents;
        assert!(ts_entities.contains("email: field(Email, Unique).optional()"));
        assert!(ts_entities.contains("handle: field(Handle, Unique),"));
        assert!(ts_entities.contains("alias: field(Alias, Unique).list(Card(0, 3))"));
    }

    #[test]
    fn python_render_infers_and_applies_case_overrides() {
        let schema_text = r#"define
attribute FirstName, value string;
attribute name, value string;

# @case(PascalCase)
entity forced_class_name, owns name @key;

# @case(Python, LowerCase)
entity forced_python_lower, owns name @key;

entity FirstPerson, owns name @key;

        entity technology_company, owns name @key;"#;

        let plan = BindgenPlan::from_typeql(schema_text).unwrap();
        let mut options = BindgenOptions::default();
        options.python_metadata.entity_annotations.insert(
            "forced_class_name".to_string(),
            BTreeMap::from([("case".to_string(), serde_json::json!("PascalCase"))]),
        );
        options.python_metadata.entity_annotations.insert(
            "forced_python_lower".to_string(),
            BTreeMap::from([(
                "case".to_string(),
                serde_json::json!(["Python", "LowerCase"]),
            )]),
        );
        let package = plan.render(TargetLanguage::Python, &options);
        let attributes = package
            .file("attributes.py")
            .expect("attributes.py was generated");
        let entities = package
            .file("entities.py")
            .expect("entities.py was generated");

        assert!(
            attributes
                .contents
                .contains("from type_bridge import AttributeFlags, String, TypeNameCase")
        );
        assert!(attributes.contents.contains("class FirstName(String):"));
        assert!(
            attributes.contents.contains(
                "flags = AttributeFlags(name=\"FirstName\", case=TypeNameCase.CLASS_NAME)"
            )
        );
        assert!(entities.contents.contains("class ForcedClassName(Entity):"));
        assert!(
            entities
                .contents
                .contains("flags = TypeFlags(case=TypeNameCase.CLASS_NAME)")
        );
        assert!(
            entities
                .contents
                .contains("class ForcedPythonLower(Entity):")
        );
        assert!(
            entities
                .contents
                .contains("flags = TypeFlags(case=TypeNameCase.LOWERCASE)")
        );
        assert!(entities.contents.contains("class FirstPerson(Entity):"));
        assert!(
            entities
                .contents
                .contains("class TechnologyCompany(Entity):")
        );
        assert!(
            entities
                .contents
                .contains("flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)")
        );
        assert!(entities.contents.contains("class FirstPerson(Entity):"));
        assert!(entities.contents.contains("flags = TypeFlags()"));
    }
}
