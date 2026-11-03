mod render;
mod reserved;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionConfig, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::{ResolvedSchema, VerifiedSchemaAuthority};

use crate::{GeneratedPackage, embedded_authority, invalid};

const PACKAGE_JSON: &[u8] = include_bytes!("package.json");
const RUNTIME_SOURCE: &[u8] = include_bytes!("runtime.ts");
const TSCONFIG_JSON: &[u8] = include_bytes!("tsconfig.json");

const PACKAGE_JSON_ID: &str = "typebridge.generator.typescript.package-json";
const RUNTIME_SOURCE_ID: &str = "typebridge.generator.typescript.runtime-source";
const TSCONFIG_JSON_ID: &str = "typebridge.generator.typescript.tsconfig-json";

/// TypeScript emitter with one native runtime evidence ledger.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptEmitter;

impl TypeScriptEmitter {
    /// Construct the emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return the native projection-handler evidence.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::typescript_v2()]
    }

    /// Return the same native handler evidence for every resolved schema.
    #[must_use]
    pub fn generator_handlers_for(&self, _schema: &ResolvedSchema) -> Vec<ProjectionHandler> {
        self.generator_handlers()
    }

    /// Hash every fixed native resource consumed by emission.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.runtime_resources()
    }

    /// Hash the same fixed native resources for every resolved schema.
    pub fn code_resources_for(
        &self,
        _schema: &ResolvedSchema,
    ) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.runtime_resources()
    }

    fn runtime_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        let mut resources = vec![
            CodeResourceDigest::from_bytes(PACKAGE_JSON_ID, PACKAGE_JSON)?,
            CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE)?,
            CodeResourceDigest::from_bytes(TSCONFIG_JSON_ID, TSCONFIG_JSON)?,
        ];
        resources.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(resources)
    }

    /// Emit one deterministic ESM package bound to exact verified schema authority.
    pub fn emit(
        &self,
        projection: &RuntimeProjection,
        authority: &VerifiedSchemaAuthority,
    ) -> Result<GeneratedPackage, Diagnostic> {
        let handlers = self.generator_handlers();
        let resources = self.runtime_resources()?;
        if projection.target() != BindingTarget::TypeScript
            || projection.config().typescript_naming_policy()
                != ProjectionConfig::typescript().typescript_naming_policy()
            || projection.generator_handlers() != handlers
            || projection.code_resources() != resources
        {
            return Err(invalid(
                "typescript_emitter_evidence_mismatch",
                "projection target, handler, or resource evidence does not match this emitter",
            ));
        }
        let authority = embedded_authority(projection, authority)?;
        render::render(
            projection,
            &authority,
            RUNTIME_SOURCE,
            PACKAGE_JSON,
            TSCONFIG_JSON,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};
    use std::collections::BTreeSet;

    fn exact_source_slice<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        assert_eq!(source.matches(start).count(), 1, "non-unique start {start}");
        let offset = source.find(start).unwrap();
        let tail = &source[offset..];
        let length = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing end {end}"));
        &tail[..length]
    }

    fn identifier_calls(source: &str) -> BTreeSet<String> {
        let normalized = source.replace("<Complete>", "").replace("<object>", "");
        let bytes = normalized.as_bytes();
        let mut calls = BTreeSet::new();
        let mut offset = 0;
        while offset < bytes.len() {
            if bytes[offset].is_ascii_alphabetic() || bytes[offset] == b'_' {
                let start = offset;
                offset += 1;
                while offset < bytes.len()
                    && (bytes[offset].is_ascii_alphanumeric() || bytes[offset] == b'_')
                {
                    offset += 1;
                }
                let mut next = offset;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                if next < bytes.len() && bytes[next] == b'(' {
                    calls.insert(normalized[start..offset].to_owned());
                }
            } else {
                offset += 1;
            }
        }
        calls
    }

    #[test]
    fn native_runtime_source_is_byte_exact() {
        let resource = CodeResourceDigest::from_bytes(RUNTIME_SOURCE_ID, RUNTIME_SOURCE).unwrap();
        if let Some(output) = std::env::var_os("TYPE_BRIDGE_TYPESCRIPT_RUNTIME_CHECKPOINT_OUTPUT") {
            use std::io::Write as _;
            let mut destination = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)
                .unwrap();
            let checkpoint = serde_json::json!({
                "length": RUNTIME_SOURCE.len(),
                "sha256": format!("{:x}", Sha256::digest(RUNTIME_SOURCE)),
                "resource_fingerprint": resource.content_fingerprint().digest().to_hex(),
            });
            destination
                .write_all(&type_bridge_contract::codec::to_canonical_json(&checkpoint).unwrap())
                .unwrap();
            return;
        }
        assert_eq!(RUNTIME_SOURCE.len(), 164_111);
        assert_eq!(
            format!("{:x}", Sha256::digest(RUNTIME_SOURCE)),
            "1bbed8e95361eada7ada6e7881e5181bd9eccfd99ee4c227e7c2d06c0bba23ac"
        );
        assert_eq!(
            resource.content_fingerprint().digest().to_hex(),
            "04a84a329bd13b4f41366a7c1635636da3b2eb58d023d7a0a2b7a7b82854529d"
        );
    }

    #[test]
    fn ordered_runtime_source_installs_whole_create_validation() {
        let source = std::str::from_utf8(RUNTIME_SOURCE).unwrap().to_owned();

        assert!(!source.contains("native.insertJson("));
        assert!(!source.contains("native.putJson("));
        assert!(!source.contains("native.updateJson("));
        assert!(source.contains("state.projection.materializeMatchThingProjected(thing)"));
        assert!(source.contains("export function defineOrderedModel<"));
        assert!(source.contains("TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 4"));
        assert!(source.contains("export type ProjectedBatchUpdate<Complete>"));
        assert!(source.contains("export interface OrderedProjectedModelManager<"));
        assert!(source.contains("export interface ProjectedModelFilter<"));
        assert!(source.contains("export class DirectConnectionPolicy"));
        assert!(source.contains("readonly #password: string;"));
        assert!(source.contains("DirectConnectionPolicy([REDACTED])"));
        assert!(source.contains("projection.connectDirect({"));
        // The base runtime imports the structured-error wrapper once.
        assert_eq!(source.matches("  projectedManagerNativeCall,\n").count(), 1);
        assert!(source.contains("const entries = Object.entries(filters);"));
        assert!(source.contains("orderedCompatibilityPredicates(typeKey, entries)"));
        assert!(!source.contains("orderedCompatibilityPredicates(typeKey, filters)"));
        assert!(source.contains("return JSON.parse(field.attribute) === name;"));
        assert!(
            source.contains("projectedManagerNativeCall(() => native.rejectForeignFieldToken())")
        );
        assert!(source.contains("export type OrderedModelToken<"));
        assert!(source.contains("native.insertManyProjected<Complete>"));
        assert!(source.contains("native.putManyProjected<Complete>"));
        assert!(source.contains("native.updateManyProjected<Complete>"));
        assert!(source.contains("native.deleteManyProjected("));
        assert!(source.contains("projectedBatchMaterializer: materializeProjectedBatch"));
        assert!(source.contains("function canonicalOrderedTemporalIso(iso: string): string"));
        assert!(
            source.contains("function lowerOrderedProjectedValue(value: unknown): ProjectedWire")
        );
        assert!(source.contains("const wire = lowerOrderedProjectedValue(instance)"));
        assert!(source.contains("JSON.stringify(lowerOrderedProjectedValue(result))"));
        assert!(
            source.contains("JSON.stringify(orderedScalarToWire(definition.valueType, input))")
        );
        assert!(source.contains("kind: \"root\" as const"));
        assert!(source.contains("kind: \"role\" as const"));
        assert!(source.contains("Parameters<NativeProjectedManager[\"insertProjected\"]>"));
        assert!(source.contains("const attributeTypeKeys = new Map<string, string>()"));
        assert!(
            source.contains("attributeTypeKeys.set(JSON.stringify(label), definition.typeKey)")
        );
        assert!(source.contains("Object.getOwnPropertyDescriptors(original)"));
        assert!(source.contains("requireProjection().validateCreateJson("));
        assert!(source.contains("descriptors[HYDRATE_COMPLETE_BRAND]"));
        assert!(source.contains("requireProjection().validateHydratedAttributeValueJson("));
        assert!(source.contains("requireProjection().validateThingJson("));
        assert!(!source.contains("materializeOrderedThing"));
        assert!(source.contains("rejectGeneratedTokenPackageMismatch("));
        let successor = &source[source
            .find("/** Canonical collection semantics shared by every generated package. */")
            .unwrap()..];
        assert!(!successor.contains("legacy.insertMany(instances)"));
        assert!(!successor.contains("legacy.putMany(instances)"));
        let projection = successor
            .find("const projection = installRuntimeProjection({")
            .unwrap();
        let authority = successor
            .find("const authority = installGeneratedSchemaAuthority({")
            .unwrap();
        assert!(projection < authority);
    }

    #[test]
    fn ordered_batch_materializer_transitive_slice_is_pure_and_closed() {
        let source = std::str::from_utf8(RUNTIME_SOURCE).unwrap().to_owned();
        let slices = [
            exact_source_slice(
                &source,
                "function assertRecord(",
                "\n\nfunction validateMultiplicity(",
            ),
            exact_source_slice(
                &source,
                "function validateMultiplicity(",
                "\n\nfunction validateAccepted(",
            ),
            exact_source_slice(
                &source,
                "function validateAccepted(",
                "\n\nfunction validateScalar(",
            ),
            exact_source_slice(
                &source,
                "function validateScalar(",
                "\n\nfunction freezeValue(",
            ),
            exact_source_slice(
                &source,
                "function snapshotDeleted(",
                "\n\nfunction retainDeletedSnapshot<",
            ),
            exact_source_slice(
                &source,
                "function parseProjectedWire(",
                "\n\nfunction hydrateProjectedValue(",
            ),
            exact_source_slice(
                &source,
                "function scalarFromWire(",
                "\n\nfunction isScalarValueType(",
            ),
            exact_source_slice(
                &source,
                "function isScalarValueType(",
                "\n\ndeclare const SELECTION_BRAND",
            ),
            exact_source_slice(
                &source,
                "function validateProjectedBatchField(",
                "\n\nfunction hydrateProjectedBatchValue(",
            ),
            exact_source_slice(
                &source,
                "function hydrateProjectedBatchValue(",
                "\n\nfunction hydrateProjectedBatchMember(",
            ),
            exact_source_slice(
                &source,
                "function hydrateProjectedBatchMember(",
                "\n\nfunction hydrateProjectedBatchWire<",
            ),
            exact_source_slice(
                &source,
                "function hydrateProjectedBatchWire<",
                "\n\nfunction retainProjectedProofs(",
            ),
            exact_source_slice(
                &source,
                "function retainProjectedProofs(",
                "\n\nconst materializeProjectedBatch:",
            ),
            exact_source_slice(
                &source,
                "const materializeProjectedBatch:",
                "\n\n/** @internal Install exact native evidence",
            ),
        ];
        let graph = slices.join("\n");

        assert_eq!(
            identifier_calls(&graph),
            BTreeSet::from([
                "BigInt".to_owned(),
                "Date".to_owned(),
                "TypeError".to_owned(),
                "assertRecord".to_owned(),
                "defineProperty".to_owned(),
                "defineDeletedState".to_owned(),
                "for".to_owned(),
                "forEach".to_owned(),
                "freeze".to_owned(),
                "get".to_owned(),
                "getOwnPropertyDescriptor".to_owned(),
                "getTime".to_owned(),
                "hydrateProjectedBatchMember".to_owned(),
                "hydrateProjectedBatchValue".to_owned(),
                "hydrateProjectedBatchWire".to_owned(),
                "hasOwn".to_owned(),
                "if".to_owned(),
                "includes".to_owned(),
                "isArray".to_owned(),
                "isFinite".to_owned(),
                "isScalarValueType".to_owned(),
                "keys".to_owned(),
                "map".to_owned(),
                "parse".to_owned(),
                "parseProjectedWire".to_owned(),
                "projectedBatchDeleted".to_owned(),
                "push".to_owned(),
                "retainProjectedProofs".to_owned(),
                "return".to_owned(),
                "roleProof".to_owned(),
                "rootProof".to_owned(),
                "scalarFromWire".to_owned(),
                "set".to_owned(),
                "some".to_owned(),
                "snapshotDeleted".to_owned(),
                "switch".to_owned(),
                "validateAccepted".to_owned(),
                "validateMultiplicity".to_owned(),
                "validateProjectedBatchField".to_owned(),
                "validateScalar".to_owned(),
            ])
        );
        for forbidden in [
            "requireProjection",
            "validateAttributeValueJson",
            "validateFieldValueJson",
            "validateHydratedAttributeValueJson",
            "validateThingJson",
            "validateOwnedMember",
            "canonicalOrderedTemporalIso",
            "orderedScalarToWire",
            "lowerOrderedProjectedValue",
            "manager(",
            ".manager(",
            "session(",
            ".session(",
            "query(",
            ".query(",
            "transaction(",
            ".transaction(",
            "provider(",
            ".provider(",
            "Promise",
            "async ",
            "await ",
        ] {
            assert!(
                !graph.contains(forbidden),
                "forbidden batch call {forbidden}"
            );
        }
        for required_guard in [
            "wire.iid !== null",
            "wire.value.valueType !== definition.valueType",
            "receivedNames.length !== names.length",
            "names.some((name) => !Object.hasOwn(wire.values, name))",
            "native complete thing wire has no IID",
            "attributeTypeKeys.get(field.attribute)",
            "candidate[\"__typebridgeModel\"] !== attributeTypeKey",
            "validateMultiplicity(member.name, value, member.multiplicity)",
            "validateAccepted(member.name, value, member.accepted)",
            "validateProjectedBatchField(definition, member.name, value)",
        ] {
            assert!(
                graph.contains(required_guard),
                "missing wire guard {required_guard}"
            );
        }

        let materializer = slices.last().unwrap();
        assert!(materializer.contains(
            "() => Object.freeze({\n      kind: \"root\" as const,\n      authority,\n      row: ordinal,\n    })"
        ));
        assert!(materializer.contains(
            "(roleName, playerIndex) => Object.freeze({\n      kind: \"role\" as const,\n      authority,\n      row: ordinal,\n      roleName,\n      playerIndex,\n    })"
        ));
        let retained = slices[slices.len() - 2];
        let first_set = retained.find("projectedFacadeProofs.set(").unwrap();
        assert!(retained.rfind("retained.push(").unwrap() < first_set);
        assert!(retained.rfind("rootProof()").unwrap() < first_set);
        assert!(retained.rfind("roleProof(").unwrap() < first_set);
        assert_eq!(retained.matches("projectedFacadeProofs.set(").count(), 1);

        assert_eq!(source.matches("materializeProjectedBatch").count(), 2);
        assert_eq!(source.matches("projectedBatchMaterializer").count(), 2);
        for call in [
            "native.insertManyProjected<Complete>(instances.length, (index) => {",
            "native.putManyProjected<Complete>(instances.length, (index) => {",
            "native.updateManyProjected<Complete>(updates.length, (index) => {",
            "native.deleteManyProjected(iids.length, (index) => {",
        ] {
            assert!(
                source.contains(call),
                "missing rowCount + rowAt call {call}"
            );
        }
    }

    #[test]
    fn native_runtime_handler_and_resources_are_schema_independent() {
        let emitter = TypeScriptEmitter::new();
        assert_eq!(
            emitter.generator_handlers(),
            [ProjectionHandler::typescript_v2()]
        );
        assert_eq!(emitter.code_resources().unwrap().len(), 3);
    }
}
