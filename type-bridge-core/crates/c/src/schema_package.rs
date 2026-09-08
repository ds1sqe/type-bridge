use std::sync::Arc;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::limits::{
    MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::projection::BindingTarget;
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::encode_declared_schema;
use type_bridge_schema::{
    decode_schema_authority, project, schema_authority_capability_vocabulary,
};
use type_bridge_schema_codegen::CEmitter;

use crate::abi::{
    SCHEMA_PACKAGE_CHUNK_BYTES_MAX, SCHEMA_PACKAGE_HOSTED_OBJECT_BYTES_MAX, SchemaPackageState,
    TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION, TypeBridgeChunkedByteViewV1, TypeBridgeSchemaPackage,
    TypeBridgeSchemaPackageChunkedDescriptorV1, TypeBridgeSchemaPackageDescriptorV1,
    TypeBridgeStatus, invalid_view,
};
use crate::allocation::{AllocationSite, try_reserve};
use crate::diagnostic::{authority_diagnostics, stable};

pub(crate) fn open(
    descriptor: TypeBridgeSchemaPackageDescriptorV1,
) -> Result<TypeBridgeSchemaPackage, (TypeBridgeStatus, Vec<Diagnostic>)> {
    if descriptor.struct_size as usize != size_of::<TypeBridgeSchemaPackageDescriptorV1>()
        || descriptor.reserved != [0; 4]
    {
        return Err(rejected(stable(
            DiagnosticCategory::InvalidContract,
            "c_schema_descriptor_layout_mismatch",
            "C schema descriptor size or reserved fields are invalid",
        )));
    }
    if descriptor.abi_major != crate::abi::ABI_MAJOR
        || descriptor.abi_minor != crate::abi::ABI_MINOR
    {
        return Err(rejected(stable(
            DiagnosticCategory::UnsupportedCapability,
            "c_schema_descriptor_abi_unsupported",
            "C schema descriptor requires an unsupported ABI version",
        )));
    }

    let authority_bytes = unsafe {
        descriptor
            .schema_authority_json
            .snapshot("schema_authority_json", MAX_CANONICAL_BYTES)
    }?;
    let declared_bytes = unsafe {
        descriptor
            .declared_schema_json
            .snapshot("declared_schema_json", MAX_CANONICAL_BYTES)
    }?;
    let projection_bytes = unsafe {
        descriptor
            .runtime_projection_json
            .snapshot("runtime_projection_json", MAX_CANONICAL_BYTES)
    }?;
    let semantic_bytes = unsafe {
        descriptor
            .semantic_fingerprint_json
            .snapshot("semantic_fingerprint_json", MAX_CANONICAL_BYTES)
    }?;
    let binding_bytes = unsafe {
        descriptor
            .binding_fingerprint_json
            .snapshot("binding_fingerprint_json", MAX_CANONICAL_BYTES)
    }?;
    let scope_bytes = unsafe {
        descriptor
            .managed_scope
            .snapshot("managed_scope", MAX_CANONICAL_STRING_BYTES)
    }?;
    let profile_bytes = unsafe {
        descriptor
            .semantic_profile
            .snapshot("semantic_profile", MAX_CANONICAL_STRING_BYTES)
    }?;

    open_owned(
        authority_bytes,
        declared_bytes,
        projection_bytes,
        semantic_bytes,
        binding_bytes,
        scope_bytes,
        profile_bytes,
    )
}

pub(crate) unsafe fn open_chunked(
    descriptor: TypeBridgeSchemaPackageChunkedDescriptorV1,
) -> Result<TypeBridgeSchemaPackage, (TypeBridgeStatus, Vec<Diagnostic>)> {
    if descriptor.struct_size as usize != size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>()
        || descriptor.reserved0 != 0
        || descriptor.reserved != [0; 4]
    {
        return Err(rejected(stable(
            DiagnosticCategory::InvalidContract,
            "c_schema_chunked_descriptor_layout_mismatch",
            "C chunked schema descriptor size or reserved fields are invalid",
        )));
    }
    if descriptor.abi_major != crate::abi::ABI_MAJOR
        || descriptor.abi_minor != crate::abi::ABI_MINOR
    {
        return Err(rejected(stable(
            DiagnosticCategory::UnsupportedCapability,
            "c_schema_descriptor_abi_unsupported",
            "C schema descriptor requires an unsupported ABI version",
        )));
    }
    let views = [
        (
            descriptor.schema_authority_json,
            "schema_authority_json",
            MAX_CANONICAL_BYTES,
        ),
        (
            descriptor.declared_schema_json,
            "declared_schema_json",
            MAX_CANONICAL_BYTES,
        ),
        (
            descriptor.runtime_projection_json,
            "runtime_projection_json",
            MAX_CANONICAL_BYTES,
        ),
        (
            descriptor.semantic_fingerprint_json,
            "semantic_fingerprint_json",
            MAX_CANONICAL_BYTES,
        ),
        (
            descriptor.binding_fingerprint_json,
            "binding_fingerprint_json",
            MAX_CANONICAL_BYTES,
        ),
        (
            descriptor.managed_scope,
            "managed_scope",
            MAX_CANONICAL_STRING_BYTES,
        ),
        (
            descriptor.semantic_profile,
            "semantic_profile",
            MAX_CANONICAL_STRING_BYTES,
        ),
    ];
    let aggregate_chunks = views.iter().try_fold(0_usize, |total, (view, _, _)| {
        total.checked_add(view.chunk_count)
    });
    if aggregate_chunks.is_none_or(|count| count > MAX_CANONICAL_COLLECTION_LEN) {
        return Err(rejected(stable(
            DiagnosticCategory::ResourceLimit,
            "c_schema_chunk_count_limit_exceeded",
            "C chunked schema descriptor exceeds its aggregate chunk ceiling",
        )));
    }
    let mut assembled = Vec::new();
    try_reserve(
        &mut assembled,
        views.len(),
        AllocationSite::SchemaPackageChunkAssembly,
    )
    .map_err(|_| {
        rejected(stable(
            DiagnosticCategory::ResourceLimit,
            "c_schema_chunk_allocation_failed",
            "The C runtime could not allocate bounded schema chunk storage",
        ))
    })?;
    for (view, field, maximum) in views {
        // SAFETY: the exported preflight established complete readable table and data ranges.
        assembled.push(unsafe { snapshot_chunked(view, field, maximum) }?);
    }
    let [
        authority_bytes,
        declared_bytes,
        projection_bytes,
        semantic_bytes,
        binding_bytes,
        scope_bytes,
        profile_bytes,
    ] = assembled
        .try_into()
        .expect("the fixed seven-view assembly has exact length");
    open_owned(
        authority_bytes,
        declared_bytes,
        projection_bytes,
        semantic_bytes,
        binding_bytes,
        scope_bytes,
        profile_bytes,
    )
}

unsafe fn snapshot_chunked(
    view: TypeBridgeChunkedByteViewV1,
    field: &'static str,
    maximum: usize,
) -> Result<Vec<u8>, (TypeBridgeStatus, Vec<Diagnostic>)> {
    if view.struct_size as usize != size_of::<TypeBridgeChunkedByteViewV1>()
        || view.version != TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION
        || view.reserved != [0; 4]
        || view.chunk_count == 0
        || view.chunk_count > MAX_CANONICAL_COLLECTION_LEN
        || view.chunks.is_null()
        || view.total_length == 0
    {
        return Err(invalid_view(
            field,
            "C chunked schema descriptor view layout is invalid",
        ));
    }
    if view.total_length > maximum {
        return Err((
            TypeBridgeStatus::ResourceLimit,
            vec![
                stable(
                    DiagnosticCategory::ResourceLimit,
                    "c_schema_descriptor_byte_limit_exceeded",
                    "C schema descriptor byte view exceeds its stable limit",
                )
                .at(
                    type_bridge_contract::diagnostic::DiagnosticPathSegment::Field(
                        field.to_owned(),
                    ),
                ),
            ],
        ));
    }
    let table_bytes = view
        .chunk_count
        .checked_mul(size_of::<crate::abi::TypeBridgeByteView>())
        .filter(|bytes| *bytes <= SCHEMA_PACKAGE_HOSTED_OBJECT_BYTES_MAX)
        .ok_or_else(|| {
            rejected(stable(
                DiagnosticCategory::ResourceLimit,
                "c_schema_chunk_table_limit_exceeded",
                "C chunked schema descriptor table exceeds its hosted-object ceiling",
            ))
        })?;
    debug_assert_ne!(table_bytes, 0);
    let mut output = Vec::new();
    try_reserve(
        &mut output,
        view.total_length,
        AllocationSite::SchemaPackageChunkAssembly,
    )
    .map_err(|_| {
        rejected(stable(
            DiagnosticCategory::ResourceLimit,
            "c_schema_chunk_allocation_failed",
            "The C runtime could not allocate bounded schema chunk storage",
        ))
    })?;
    let mut total = 0_usize;
    for index in 0..view.chunk_count {
        // SAFETY: preflight proved the complete bounded chunk table readable.
        let chunk = unsafe { view.chunks.add(index).read_unaligned() };
        if chunk.length == 0
            || chunk.length > SCHEMA_PACKAGE_CHUNK_BYTES_MAX
            || chunk.data.is_null()
        {
            return Err(invalid_view(
                field,
                "C chunked schema descriptor chunks must be non-empty, byte-bounded, and non-null",
            ));
        }
        total = total.checked_add(chunk.length).ok_or_else(|| {
            rejected(stable(
                DiagnosticCategory::ResourceLimit,
                "c_schema_descriptor_byte_limit_exceeded",
                "C schema descriptor byte view exceeds its stable limit",
            ))
        })?;
        if total > view.total_length || total > maximum {
            return Err(invalid_view(
                field,
                "C chunked schema descriptor total length is inconsistent",
            ));
        }
        // SAFETY: preflight proved each complete nonempty data range readable.
        output.extend_from_slice(unsafe { std::slice::from_raw_parts(chunk.data, chunk.length) });
    }
    if total != view.total_length {
        return Err(invalid_view(
            field,
            "C chunked schema descriptor total length is inconsistent",
        ));
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn open_owned(
    authority_bytes: Vec<u8>,
    declared_bytes: Vec<u8>,
    projection_bytes: Vec<u8>,
    semantic_bytes: Vec<u8>,
    binding_bytes: Vec<u8>,
    scope_bytes: Vec<u8>,
    profile_bytes: Vec<u8>,
) -> Result<TypeBridgeSchemaPackage, (TypeBridgeStatus, Vec<Diagnostic>)> {
    let authority =
        decode_schema_authority(&authority_bytes, &schema_authority_capability_vocabulary())
            .map_err(|error| {
                let diagnostics = authority_diagnostics(error);
                (status_for(&diagnostics), diagnostics)
            })?;
    let projection =
        decode_runtime_projection_verified(&projection_bytes, &semantic_bytes, &binding_bytes)
            .map_err(rejected)?;

    let emitter = CEmitter::new();
    let expected_handlers = emitter.generator_handlers_for(authority.resolved_schema());
    let expected_resources = emitter
        .code_resources_for(authority.resolved_schema())
        .map_err(rejected)?;
    let recomputed = project(
        authority.resolved_schema(),
        BindingTarget::C,
        projection.config(),
        &expected_handlers,
        &expected_resources,
    )
    .map_err(|errors| {
        let diagnostics = errors
            .into_vec()
            .into_iter()
            .map(|error| error.diagnostic().clone())
            .collect::<Vec<_>>();
        (status_for(&diagnostics), diagnostics)
    })?;
    let recomputed_bytes = to_canonical_json(&recomputed).map_err(rejected)?;

    if projection.target() != BindingTarget::C
        || projection.config().c_symbol_prefix().is_none()
        || projection.generator_handlers() != expected_handlers
        || projection.code_resources() != expected_resources
        || projection != recomputed
        || projection_bytes != recomputed_bytes
        || projection.semantic_fingerprint() != authority.resolved_schema().semantic_fingerprint()
        || encode_declared_schema(authority.declared_schema()).map_err(rejected)? != declared_bytes
        || authority.managed_scope().id().as_str().as_bytes() != scope_bytes
        || authority.semantic_profile().id().as_str().as_bytes() != profile_bytes
        || to_canonical_json(projection.semantic_fingerprint()).map_err(rejected)? != semantic_bytes
        || to_canonical_json(projection.projection_fingerprint()).map_err(rejected)?
            != binding_bytes
    {
        return Err(rejected(stable(
            DiagnosticCategory::Integrity,
            "c_schema_package_evidence_mismatch",
            "generated C schema package evidence does not describe one exact authority and projection",
        )));
    }

    let declared_schema_identity = authority
        .resolved_schema()
        .declared_identity_fingerprint()
        .clone();
    let installed_projection =
        type_bridge_orm::InstalledRuntimeProjection::try_new(projection.clone())
            .map_err(|_| {
                rejected(stable(
                    DiagnosticCategory::Integrity,
                    "c_schema_package_runtime_projection_invalid",
                    "generated C runtime projection could not be installed",
                ))
            })?
            .with_declared_schema_identity(declared_schema_identity);

    Ok(TypeBridgeSchemaPackage {
        state: Arc::new(SchemaPackageState {
            authority_json: authority_bytes,
            projection_json: projection_bytes,
            semantic_fingerprint_json: semantic_bytes,
            binding_fingerprint_json: binding_bytes,
            managed_scope: scope_bytes,
            semantic_profile: profile_bytes,
            _authority: authority,
            _projection: projection,
            installed_projection,
        }),
    })
}

fn rejected(error: Diagnostic) -> (TypeBridgeStatus, Vec<Diagnostic>) {
    let diagnostics = vec![error];
    (status_for(&diagnostics), diagnostics)
}

pub(crate) fn status_for(diagnostics: &[Diagnostic]) -> TypeBridgeStatus {
    if diagnostics
        .iter()
        .any(|error| error.category() == DiagnosticCategory::ResourceLimit)
    {
        TypeBridgeStatus::ResourceLimit
    } else if diagnostics
        .iter()
        .any(|error| error.category() == DiagnosticCategory::UnsupportedCapability)
    {
        TypeBridgeStatus::Unsupported
    } else {
        TypeBridgeStatus::SchemaPackageRejected
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;
    use crate::abi::{ABI_MAJOR, ABI_MINOR, TypeBridgeByteView};
    use crate::allocation::inject_failure;

    #[test]
    fn chunk_assembly_failure_after_partial_progress_is_deterministic_and_cleaned_up() {
        let byte = *b"x";
        let chunk = [TypeBridgeByteView {
            data: byte.as_ptr(),
            length: byte.len(),
        }];
        let view = TypeBridgeChunkedByteViewV1 {
            struct_size: size_of::<TypeBridgeChunkedByteViewV1>() as u32,
            version: TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION,
            chunks: chunk.as_ptr(),
            chunk_count: chunk.len(),
            total_length: byte.len(),
            reserved: [0; 4],
        };
        let descriptor = TypeBridgeSchemaPackageChunkedDescriptorV1 {
            struct_size: size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>() as u32,
            abi_major: ABI_MAJOR,
            abi_minor: ABI_MINOR,
            reserved0: 0,
            schema_authority_json: view,
            declared_schema_json: view,
            runtime_projection_json: view,
            semantic_fingerprint_json: view,
            binding_fingerprint_json: view,
            managed_scope: view,
            semantic_profile: view,
            reserved: [0; 4],
        };

        // The fixed outer assembly and two completed byte vectors are dropped
        // when the fourth allocation checkpoint rejects the third view.
        let _failure = inject_failure(AllocationSite::SchemaPackageChunkAssembly, 3);
        // SAFETY: all seven view tables and data bytes remain live for the call.
        let (status, diagnostics) = match unsafe { open_chunked(descriptor) } {
            Ok(_) => panic!("injected chunk assembly failure must be reported"),
            Err(error) => error,
        };
        assert_eq!(status, TypeBridgeStatus::ResourceLimit);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code().as_str(),
            "c_schema_chunk_allocation_failed"
        );

        // The one-shot failpoint was consumed and no partial assembly escaped.
        // SAFETY: the same retained immutable descriptor remains live.
        let (_, diagnostics) = match unsafe { open_chunked(descriptor) } {
            Ok(_) => panic!("arbitrary bytes must reach ordinary schema rejection"),
            Err(error) => error,
        };
        assert_ne!(
            diagnostics[0].code().as_str(),
            "c_schema_chunk_allocation_failed"
        );
    }
}
