//! Captured authority and limits for generated C recordings.
use super::*;
use crate::generated_preflight::GENERATED_INPUT_PLAN_OPTIONS;
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token, validate_token};
use crate::query::TypeBridgeQueryLimitOverrideV1;
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticName, SdkDiagnosticPathSegment};

pub(super) struct ModelRegistration {
    pub(super) type_id: u32,
    fields: Vec<u32>,
    roles: Vec<u32>,
}

/// Version of the generated recording options layout.
pub const TYPE_BRIDGE_PLAN_OPTIONS_VERSION: u32 = 1;

/// Optional named limit overrides, captured when a generated recording opens.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgePlanOptionsV1 {
    /// Exact size of this structure.
    pub struct_size: u32,
    /// TYPE_BRIDGE_PLAN_OPTIONS_VERSION.
    pub version: u32,
    /// Borrowed overrides, read only for the open call.
    pub query_limits: *const TypeBridgeQueryLimitOverrideV1,
    /// Number of override entries.
    pub query_limit_count: usize,
    /// Reserved zero words.
    pub reserved: [u64; 4],
}

impl TypeBridgePlanOptionsV1 {
    /// Fence all nested override ranges before any caller output is written.
    /// # Safety
    /// Non-null arrays/views are readable for their declared lengths.
    pub(crate) unsafe fn check_borrowed_ranges(
        &self,
        outputs: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        // SAFETY: the recording retains the same caller-readable override graph.
        unsafe {
            crate::query::check_query_limit_override_ranges(
                outputs,
                self.query_limits,
                self.query_limit_count,
            )
        }
    }
}

/// Open a generated recording retaining its package and selected limits once.
/// NULL options select canonical limits; malformed options open nothing.
///
/// # Safety
/// Package/options are live and immutable during the call. The owner output is
/// writable, possibly unaligned, and may not overlap any input or retained bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_new_with_package(
    package: *const TypeBridgeSchemaPackage,
    options: *const TypeBridgePlanOptionsV1,
    out_builder: *mut *mut TypeBridgePlanBuilder,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[(
            out_builder.cast(),
            size_of::<*mut TypeBridgePlanBuilder>(),
        )]) {
            Ok(outputs) => outputs,
            Err(status) => return status,
        };
        if let Err(status) =
            outputs.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())
        {
            return status;
        }
        // SAFETY: complete outer objects are fenced before any nested range is inspected.
        if let Err(status) =
            unsafe { outputs.check_deep_object_kind(GENERATED_INPUT_PLAN_OPTIONS, options.cast()) }
        {
            return status;
        }
        if !package.is_null() {
            // SAFETY: the package remains live throughout the call.
            if let Err(status) = outputs.check_package_borrowed_ranges(unsafe { &*package }.state())
            {
                return status;
            }
        }
        if out_builder.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the output is writable and disjoint from every input.
        unsafe { out_builder.write_unaligned(ptr::null_mut()) };
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        let limits = if options.is_null() {
            QueryLimits::CANONICAL
        } else {
            // SAFETY: options are caller-readable and were fenced above.
            let options = unsafe { options.read_unaligned() };
            if options.struct_size as usize != size_of::<TypeBridgePlanOptionsV1>()
                || options.version != TYPE_BRIDGE_PLAN_OPTIONS_VERSION
                || options.reserved != [0; 4]
            {
                return TypeBridgeStatus::InvalidArgument;
            }
            // SAFETY: the complete override graph was checked above.
            let Some(limits) = (unsafe {
                crate::query::query_limits_from_overrides(
                    options.query_limits,
                    options.query_limit_count,
                )
            }) else {
                return TypeBridgeStatus::InvalidArgument;
            };
            limits
        };
        let mut builder = TypeBridgePlanBuilder::new();
        builder.core = PlanBuilder::with_limits(limits);
        // SAFETY: package is live, and only its immutable Arc is retained.
        builder.package = Some(Arc::clone(unsafe { &*package }.state()));
        // SAFETY: the output was fenced and initialized above.
        unsafe { out_builder.write_unaligned(Box::into_raw(Box::new(builder))) };
        TypeBridgeStatus::Ok
    })
}

/// Admit a generated model token under the recording's captured package brand.
/// This checks cached token identity; it does not revalidate a schema.
///
/// # Safety
/// The builder is live and exclusively borrowed; token/opener views are readable
/// during the call. The writable output may be unaligned and cannot alias inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_type_token(
    builder: *mut TypeBridgePlanBuilder,
    token: *const TypeBridgeProjectedTokenV1,
    opener: TypeBridgeByteView,
    out_type: *mut u32,
) -> TypeBridgeStatus {
    // SAFETY: every descriptor and input range follows the documented contract.
    if let Err(status) = unsafe {
        enter(
            builder,
            Some(out_type),
            &[
                (token.cast(), size_of::<TypeBridgeProjectedTokenV1>()),
                view_range(opener),
            ],
        )
    } {
        return status;
    }
    guarded(|| {
        // SAFETY: the live builder was fenced and is exclusively borrowed.
        let builder = unsafe { &mut *builder };
        if let Err(status) = builder.require_open() {
            return status;
        }
        let Some(package) = &builder.package else {
            // SAFETY: the opener name view is readable and only copied on this error path.
            let opener = if opener.length <= 255 {
                unsafe { text(opener) }.filter(|name| !name.is_empty())
            } else {
                None
            };
            let opener = opener.unwrap_or_else(|| "the generated package_plan_open".to_owned());
            let operation = SdkDiagnosticName::new(opener).unwrap_or_else(|_| {
                SdkDiagnosticName::new("package_plan_open").expect("static name")
            });
            builder.fail(
                invalid(
                    "plan_context_missing",
                    "The raw builder is unbound; use the generated package_plan_open constructor",
                )
                .try_with_path_prefix([SdkDiagnosticPathSegment::Argument(operation)])
                .expect("one bounded path segment"),
            );
            return TypeBridgeStatus::InvalidArgument;
        };
        // SAFETY: the generated model token remains readable for this call.
        let checked = match unsafe {
            validate_token(
                package,
                token,
                type_bridge_contract::projection::ProjectedTokenKind::Model,
            )
        } {
            Ok(token) => token,
            Err(error) => {
                let status = crate::execution_diagnostic::status_for(&error);
                builder.fail(error);
                return status;
            }
        };
        if let Some(Some(model)) = builder.models.get(checked.ordinal as usize) {
            // SAFETY: the output was fenced and cleared above.
            unsafe { out_type.write_unaligned(model.type_id) };
            return idioms::statement_status(builder);
        }
        // SAFETY: the shared resolver owns canonical ordinal resolution on a cache miss.
        let model = match unsafe { resolve_model_token(package, token) } {
            Ok(model) => model,
            Err(error) => {
                let status = crate::execution_diagnostic::status_for(&error);
                builder.fail(error);
                return status;
            }
        };
        let projection = &package._projection.models()[&model];
        let fields = projection
            .query_tokens()
            .fields()
            .values()
            .map(|field| push(&mut builder.fields, Some(field.id().clone())))
            .collect();
        let roles = projection
            .query_tokens()
            .roles()
            .keys()
            .map(|role| push(&mut builder.roles, Some(role.clone())))
            .collect();
        let id = push(&mut builder.types, Some(model));
        builder.models.resize_with(
            builder.models.len().max(checked.ordinal as usize + 1),
            || None,
        );
        builder.models[checked.ordinal as usize] = Some(ModelRegistration {
            type_id: id,
            fields,
            roles,
        });
        // SAFETY: the output was fenced and cleared above.
        unsafe { out_type.write_unaligned(id) };
        idioms::statement_status(builder)
    })
}

macro_rules! model_identity {
    ($name:ident, $member:ident) => {
        /// Look up one previously admitted model member without recording or allocating.
        /// Returns UINT32_MAX and records a typed fault for an unavailable member.
        /// # Safety
        /// A non-null builder is live and exclusively borrowed during the call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            builder: *mut TypeBridgePlanBuilder,
            model: u32,
            member: u32,
        ) -> u32 {
            if builder.is_null() {
                return u32::MAX;
            }
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: the caller lends the live builder exclusively.
                let builder = unsafe { &mut *builder };
                match builder
                    .models
                    .get(model as usize)
                    .and_then(Option::as_ref)
                    .and_then(|model| model.$member.get(member as usize))
                    .copied()
                {
                    Some(id) => id,
                    None => {
                        builder.fail(id_unknown());
                        u32::MAX
                    }
                }
            }))
            .unwrap_or(u32::MAX)
        }
    };
}
model_identity!(type_bridge_plan_builder_model_field, fields);
model_identity!(type_bridge_plan_builder_model_role, roles);

/// Open a generated recording after verifying the compiler's pinned C view layout.
/// Unsupported host ABIs and digest mismatches are refused before any view allocation.
/// # Safety
/// Package/options and the digest bytes are live and readable. Both writable,
/// possibly unaligned owner slots must be disjoint from all inputs and retained bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_new_with_layout(
    package: *const TypeBridgeSchemaPackage,
    options: *const TypeBridgePlanOptionsV1,
    digest: TypeBridgeByteView,
    out_builder: *mut *mut TypeBridgePlanBuilder,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let inputs = |outputs: &DirectOutputPreflight| {
        outputs.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())?;
        // SAFETY: the full options graph follows the same contract as new_with_package.
        unsafe { outputs.check_deep_object_kind(GENERATED_INPUT_PLAN_OPTIONS, options.cast()) }?;
        if !package.is_null() {
            // SAFETY: the complete live package is fenced above.
            outputs.check_package_borrowed_ranges(unsafe { &*package }.state())?;
        }
        if !digest.data.is_null() {
            outputs.check_bytes(digest.data.cast(), digest.length)?;
        }
        Ok(())
    };
    // SAFETY: outputs are checked against the entire borrowed input graph first.
    if let Err(status) = unsafe { prepare_handle_outputs(out_builder, out_diagnostics, inputs) } {
        return status;
    }
    if package.is_null() || digest.data.is_null() || digest.length != 32 {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        if !cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_pointer_width = "64",
            target_endian = "little"
        )) {
            use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticCode, SdkDiagnosticMessage};
            return return_execution_error(
                SdkExecutionDiagnostic::unsupported_capability(
                    SdkDiagnosticCode::new("c_plan_view_abi_unsupported").expect("static code"),
                    SdkDiagnosticMessage::new(
                        "Generated model views require the supported Linux x86_64 C ABI",
                    )
                    .expect("static message"),
                ),
                out_diagnostics,
            );
        }
        // SAFETY: complete live package and fixed-length digest are checked above.
        let package_state = unsafe { &*package }.state();
        let layouts = match type_bridge_contract::projection::c_view::CViewLayouts::for_projection(
            &package_state._projection,
        ) {
            Ok(layouts) => layouts,
            Err(error) => {
                return return_execution_error(
                    contract_error(&error, "The captured C view layout is invalid"),
                    out_diagnostics,
                );
            }
        };
        // SAFETY: exactly 32 digest bytes remain readable for this call.
        if unsafe { std::slice::from_raw_parts(digest.data, 32) } != layouts.digest() {
            return return_execution_error(
                invalid(
                    "c_plan_view_layout_mismatch",
                    "The generated header and native runtime disagree on model-view layouts",
                ),
                out_diagnostics,
            );
        }
        // SAFETY: original inputs and output were fully fenced above; the house
        // constructor still owns option validation and captured package/limit setup.
        let status =
            unsafe { type_bridge_plan_builder_new_with_package(package, options, out_builder) };
        if status == TypeBridgeStatus::Ok {
            // SAFETY: successful constructor returned one exclusively owned live builder.
            unsafe { &mut *out_builder.read_unaligned() }.view_layouts = Some(Arc::new(layouts));
        }
        status
    })
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn layout_digest_mismatch_opens_nothing_and_returns_a_typed_diagnostic() {
        let package = crate::entity_crud::tests::package("view_digest");
        let mut digest = type_bridge_contract::projection::c_view::CViewLayouts::for_projection(
            &package.state()._projection,
        )
        .unwrap()
        .digest();
        digest[0] ^= 1;
        let mut builder = ptr::dangling_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: all readable inputs and writable owner slots are distinct and live.
        unsafe {
            assert_ne!(
                type_bridge_plan_builder_new_with_layout(
                    &package,
                    ptr::null(),
                    TypeBridgeByteView {
                        data: digest.as_ptr(),
                        length: 32
                    },
                    &mut builder,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            assert!(builder.is_null());
            let (_, code, _) = crate::entity_crud::tests::diagnostic(diagnostics);
            assert_eq!(code, "c_plan_view_layout_mismatch");
            crate::execution_diagnostic::type_bridge_execution_diagnostics_close(&mut diagnostics);
            digest[0] ^= 1;
            assert_eq!(
                type_bridge_plan_builder_new_with_layout(
                    &package,
                    ptr::null(),
                    TypeBridgeByteView {
                        data: digest.as_ptr(),
                        length: 32
                    },
                    &mut builder,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            assert!(diagnostics.is_null());
            assert!((*builder).view_layouts.is_some());
            type_bridge_plan_builder_free(&mut builder);
        }
    }
}
