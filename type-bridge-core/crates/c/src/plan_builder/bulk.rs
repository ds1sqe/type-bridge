//! One ownership table and directly consumed scalar cells for generated bulk calls.
use super::*;

/// One registered ownership and canonical scalar domain, shared by every row.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgePlanBulkColumn {
    /// Dense ownership id registered on this builder.
    pub field: u32,
    /// Canonical domain using the projected-value kind constants.
    pub value_type: i32,
}

/// Generated host-row layout shared by every row. Offsets use the C host types:
/// int64_t, double, bool, or const char* for textual domains.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgePlanBulkHostColumn {
    /// Registered ownership identity.
    pub field: u32,
    /// Projected scalar domain.
    pub value_type: i32,
    /// Byte offset of the host scalar within one row.
    pub value_offset: usize,
    /// Byte offset of bool has_field; usize::MAX means required.
    pub presence_offset: usize,
}

/// Literal storage selected by the column's scalar domain.
#[repr(C)]
#[derive(Clone, Copy)]
pub union TypeBridgePlanBulkLiteral {
    /// Signed long literal.
    pub integer: i64,
    /// Finite double literal.
    pub double_value: f64,
    /// Boolean literal, exactly zero or one.
    pub boolean: u8,
    /// Borrowed canonical UTF-8 literal for textual domains.
    pub text: TypeBridgeByteView,
}

/// Optional scalar cell; absent cells never read their literal storage.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgePlanBulkCell {
    /// Zero omits this ownership; one consumes the literal.
    pub present: u8,
    /// Literal member selected by the corresponding column.
    pub value: TypeBridgePlanBulkLiteral,
}

fn array_range<T>(
    data: *const T,
    count: usize,
) -> Result<(*const c_void, usize), TypeBridgeStatus> {
    let bytes = count
        .checked_mul(size_of::<T>())
        .filter(|bytes| *bytes <= isize::MAX as usize)
        .ok_or(TypeBridgeStatus::InvalidArgument)?;
    Ok((data.cast(), bytes))
}

unsafe fn scalar(
    cell: TypeBridgePlanBulkCell,
    domain: ValueTypeTag,
    limits: type_bridge_contract::limits::CodecLimits,
) -> Option<CanonicalValue> {
    // SAFETY: the caller initialized the union member selected by the column domain.
    unsafe {
        match domain {
            ValueTypeTag::Long => Some(CanonicalValue::Long(cell.value.integer)),
            ValueTypeTag::Double => CanonicalDouble::new(cell.value.double_value)
                .ok()
                .map(CanonicalValue::Double),
            ValueTypeTag::Boolean => match cell.value.boolean {
                0 => Some(CanonicalValue::Boolean(false)),
                1 => Some(CanonicalValue::Boolean(true)),
                _ => None,
            },
            _ => {
                let view = cell.value.text;
                if view.data.is_null()
                    || view.length > limits.max_string_bytes.min(limits.max_bytes)
                {
                    return None;
                }
                let bytes = if view.length == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(view.data, view.length)
                };
                let text = std::str::from_utf8(bytes).ok()?;
                match domain {
                    ValueTypeTag::String => CanonicalString::new_with_limits(text, limits)
                        .ok()
                        .map(CanonicalValue::String),
                    ValueTypeTag::Date => {
                        CanonicalDate::from_str(text).ok().map(CanonicalValue::Date)
                    }
                    ValueTypeTag::DateTime => CanonicalDateTime::from_str(text)
                        .ok()
                        .map(CanonicalValue::DateTime),
                    ValueTypeTag::DateTimeTz => parse_provider_datetime_tz(text)
                        .ok()
                        .map(CanonicalValue::DateTimeTz),
                    ValueTypeTag::Decimal => DecimalValue::new(text)
                        .ok()
                        .filter(|value| value.as_str() == text)
                        .map(CanonicalValue::Decimal),
                    ValueTypeTag::Duration => CanonicalDuration::from_str(text)
                        .ok()
                        .map(CanonicalValue::Duration),
                    _ => None,
                }
            }
        }
    }
}

/// Bind generated bulk values without entering the reusable literal registry.
/// Cells are row-major, exactly `row_count * column_count`; columns occur once.
/// Text views and arrays remain readable for this call; no storage is retained.
/// Empty rows with zero columns are preserved, including a zero-row bulk.
///
/// # Safety
/// The builder is live and exclusively borrowed. Arrays name their declared
/// readable lengths; present cells initialize the union member selected by their
/// column. Textual present cells name readable views. Arrays may be unaligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_model_rows(
    builder: *mut TypeBridgePlanBuilder,
    thing: u32,
    columns: *const TypeBridgePlanBulkColumn,
    column_count: usize,
    cells: *const TypeBridgePlanBulkCell,
    row_count: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        let count = match row_count.checked_mul(column_count) {
            Some(count) => count,
            None => return TypeBridgeStatus::InvalidArgument,
        };
        let ranges = match (
            array_range(columns, column_count),
            array_range(cells, count),
        ) {
            (Ok(columns), Ok(cells)) => [columns, cells],
            _ => return TypeBridgeStatus::InvalidArgument,
        };
        // SAFETY: checks array extents and the live builder before any dereference.
        if let Err(status) = unsafe { enter(builder, None, &ranges) } {
            return status;
        }
        let mut metadata = Vec::with_capacity(column_count);
        for index in 0..column_count {
            // SAFETY: the fenced column array contains this possibly unaligned element.
            metadata.push(unsafe { columns.add(index).read_unaligned() });
        }
        // Fence nested views before reading their bytes or mutating the builder.
        for index in 0..count {
            // SAFETY: the fenced cell array contains this element.
            let cell = unsafe { cells.add(index).read_unaligned() };
            if cell.present == 1 && matches!(metadata[index % column_count].value_type, 1 | 5..=9) {
                // SAFETY: present textual cells initialize their text member.
                let view = unsafe { cell.value.text };
                if (view.data.is_null() && view.length != 0) || view.length > isize::MAX as usize {
                    return TypeBridgeStatus::InvalidArgument;
                }
            }
        }
        // SAFETY: the live builder is exclusively borrowed for this call.
        let recorder = unsafe { &mut *builder };
        if let Err(status) = recorder.require_open() {
            return status;
        }
        let Some(thing) = recorder.binding(thing) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let mut owns = Vec::with_capacity(column_count);
        let mut domains = Vec::with_capacity(column_count);
        for column in metadata {
            let Some(domain) = value_type(column.value_type) else {
                recorder.fail(constant_invalid());
                return TypeBridgeStatus::InvalidArgument;
            };
            let Some(field) = recorder.field_id(column.field) else {
                return TypeBridgeStatus::InvalidArgument;
            };
            owns.push(field);
            domains.push(domain);
        }
        let mut rows = Vec::with_capacity(row_count);
        for row in 0..row_count {
            let mut values = Vec::with_capacity(column_count);
            for (column, domain) in domains.iter().enumerate() {
                // SAFETY: row-major checked multiplication stays inside the fenced array.
                let cell = unsafe { cells.add(row * column_count + column).read_unaligned() };
                let value = match cell.present {
                    0 => None,
                    1 => match unsafe { scalar(cell, *domain, recorder.core.limits().codec) } {
                        Some(value) => Some(value),
                        None => {
                            recorder.fail(value_invalid());
                            return TypeBridgeStatus::InvalidArgument;
                        }
                    },
                    _ => {
                        recorder.fail(value_invalid());
                        return TypeBridgeStatus::InvalidArgument;
                    }
                };
                values.push(value);
            }
            rows.push(values);
        }
        recorder.core.bind_model_columns(thing, owns, rows);
        recorder
            .error
            .as_ref()
            .map(crate::execution_diagnostic::status_for)
            .unwrap_or(TypeBridgeStatus::Ok)
    })
}

fn host_layout(domain: ValueTypeTag) -> (usize, usize) {
    match domain {
        ValueTypeTag::Long => (size_of::<i64>(), align_of::<i64>()),
        ValueTypeTag::Double => (size_of::<f64>(), align_of::<f64>()),
        ValueTypeTag::Boolean => (size_of::<u8>(), align_of::<u8>()),
        _ => (
            size_of::<*const std::ffi::c_char>(),
            align_of::<*const std::ffi::c_char>(),
        ),
    }
}

fn host_rows_layout_invalid(builder: &mut TypeBridgePlanBuilder) -> TypeBridgeStatus {
    builder.fail(invalid(
        "c_plan_bulk_layout_invalid",
        "The generated bulk row layout or extent is invalid",
    ));
    TypeBridgeStatus::InvalidArgument
}

/// Consume generated raw host structs directly, without a temporary cell frame.
/// Rows and their NUL-terminated strings are borrowed only during this call.
/// False presence flags never read payloads; true NULL strings are value faults.
///
/// # Safety
/// The builder is live and exclusively borrowed. Columns name column_count
/// readable descriptors. Rows name row_count readable host structs of row_size
/// bytes, with initialized boolean flags and present values. Non-null present
/// string pointers name readable NUL-terminated UTF-8 strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_plan_builder_model_host_rows(
    builder: *mut TypeBridgePlanBuilder,
    thing: u32,
    columns: *const TypeBridgePlanBulkHostColumn,
    column_count: usize,
    rows: *const c_void,
    row_count: usize,
    row_size: usize,
) -> TypeBridgeStatus {
    guarded(|| {
        if builder.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: the caller lends the live builder exclusively.
        let recorder = unsafe { &mut *builder };
        if let Err(status) = recorder.require_open() {
            return status;
        }
        let Some(bytes) = row_count
            .checked_mul(row_size)
            .filter(|bytes| *bytes <= isize::MAX as usize)
        else {
            return host_rows_layout_invalid(recorder);
        };
        let Ok(columns_range) = array_range(columns, column_count) else {
            return host_rows_layout_invalid(recorder);
        };
        if row_size == 0 || row_size > isize::MAX as usize || (row_count != 0 && rows.is_null()) {
            return host_rows_layout_invalid(recorder);
        }
        // SAFETY: the complete descriptor and row ranges are fenced before reads.
        if unsafe { enter(builder, None, &[columns_range, (rows, bytes)]) }.is_err() {
            return host_rows_layout_invalid(recorder);
        }
        let mut metadata = Vec::with_capacity(column_count);
        for index in 0..column_count {
            // SAFETY: the caller-readable descriptor array was fenced above.
            let column = unsafe { columns.add(index).read_unaligned() };
            let Some(domain) = value_type(column.value_type) else {
                return host_rows_layout_invalid(recorder);
            };
            let (size, alignment) = host_layout(domain);
            if column
                .value_offset
                .checked_add(size)
                .is_none_or(|end| end > row_size)
                || !column.value_offset.is_multiple_of(alignment)
                || !row_size.is_multiple_of(alignment)
                || (row_count != 0 && !rows.addr().is_multiple_of(alignment))
                || (column.presence_offset != usize::MAX
                    && column
                        .presence_offset
                        .checked_add(1)
                        .is_none_or(|end| end > row_size))
            {
                return host_rows_layout_invalid(recorder);
            }
            metadata.push((column, domain));
        }
        let Some(thing) = recorder.binding(thing) else {
            return TypeBridgeStatus::InvalidArgument;
        };
        let mut owns = Vec::with_capacity(column_count);
        for (column, _) in &metadata {
            let Some(field) = recorder.field_id(column.field) else {
                return TypeBridgeStatus::InvalidArgument;
            };
            owns.push(field);
        }
        let mut canonical_rows = Vec::with_capacity(row_count);
        for index in 0..row_count {
            // SAFETY: checked multiplication and row extents admit this complete row.
            let row = unsafe { rows.cast::<u8>().add(index * row_size) };
            let mut values = Vec::with_capacity(column_count);
            for (column, domain) in &metadata {
                let present = if column.presence_offset == usize::MAX {
                    1
                } else {
                    // SAFETY: the validated presence offset names one initialized bool byte.
                    unsafe { row.add(column.presence_offset).read() }
                };
                if present == 0 {
                    values.push(None);
                    continue;
                }
                if present != 1 {
                    recorder.fail(value_invalid());
                    return TypeBridgeStatus::InvalidArgument;
                }
                // SAFETY: the complete aligned host scalar fits the validated row layout.
                let literal = unsafe {
                    let value = row.add(column.value_offset);
                    match domain {
                        ValueTypeTag::Long => TypeBridgePlanBulkLiteral {
                            integer: value.cast::<i64>().read(),
                        },
                        ValueTypeTag::Double => TypeBridgePlanBulkLiteral {
                            double_value: value.cast::<f64>().read(),
                        },
                        ValueTypeTag::Boolean => TypeBridgePlanBulkLiteral {
                            boolean: value.read(),
                        },
                        _ => {
                            let text = value.cast::<*const std::ffi::c_char>().read();
                            let view = if text.is_null() {
                                TypeBridgeByteView {
                                    data: ptr::null(),
                                    length: 0,
                                }
                            } else {
                                let text = std::ffi::CStr::from_ptr(text).to_bytes();
                                TypeBridgeByteView {
                                    data: text.as_ptr(),
                                    length: text.len(),
                                }
                            };
                            TypeBridgePlanBulkLiteral { text: view }
                        }
                    }
                };
                // SAFETY: this stack cell initializes precisely the selected scalar member.
                let Some(value) = (unsafe {
                    scalar(
                        TypeBridgePlanBulkCell {
                            present,
                            value: literal,
                        },
                        *domain,
                        recorder.core.limits().codec,
                    )
                }) else {
                    recorder.fail(value_invalid());
                    return TypeBridgeStatus::InvalidArgument;
                };
                values.push(Some(value));
            }
            canonical_rows.push(values);
        }
        recorder
            .core
            .bind_model_columns(thing, owns, canonical_rows);
        idioms::statement_status(recorder)
    })
}

#[cfg(test)]
#[path = "../../tests/support/purchase_package.rs"]
pub(crate) mod purchase_package;

#[cfg(test)]
mod tests {
    #[test]
    fn bulk_strings_use_the_selected_codec_limits() {
        let text = "x".repeat(MAX_CANONICAL_STRING_BYTES + 1);
        let cell = TypeBridgePlanBulkCell {
            present: 1,
            value: TypeBridgePlanBulkLiteral {
                text: TypeBridgeByteView {
                    data: text.as_ptr(),
                    length: text.len(),
                },
            },
        };
        let mut limits = type_bridge_contract::limits::QueryLimits::CANONICAL.codec;
        // SAFETY: the initialized text union member borrows live UTF-8 bytes.
        unsafe {
            assert!(scalar(cell, ValueTypeTag::String, limits).is_none());
            limits.max_string_bytes = text.len();
            limits.max_bytes = limits.max_bytes.max(text.len());
            assert!(
                matches!(scalar(cell, ValueTypeTag::String, limits), Some(CanonicalValue::String(value)) if value.as_str() == text)
            );
        }
    }

    use super::*;

    #[test]
    fn ten_thousand_rows_use_three_registered_fields_and_no_literal_registry() {
        let mut builder = TypeBridgePlanBuilder::new();
        let model = builder.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("item".into()));
        let fields = ["item-id", "price", "stock"]
            .map(|name| builder.register_field(model, Some(name.into())));
        let thing = builder.thing(model, PlanBuilder::insert);
        let columns = [
            TypeBridgePlanBulkColumn {
                field: fields[0],
                value_type: 1,
            },
            TypeBridgePlanBulkColumn {
                field: fields[1],
                value_type: 2,
            },
            TypeBridgePlanBulkColumn {
                field: fields[2],
                value_type: 2,
            },
        ];
        let names = (0..10_000)
            .map(|index| format!("I{index}"))
            .collect::<Vec<_>>();
        let cells = names
            .iter()
            .flat_map(|name| {
                [
                    TypeBridgePlanBulkCell {
                        present: 1,
                        value: TypeBridgePlanBulkLiteral {
                            text: TypeBridgeByteView {
                                data: name.as_ptr(),
                                length: name.len(),
                            },
                        },
                    },
                    TypeBridgePlanBulkCell {
                        present: 1,
                        value: TypeBridgePlanBulkLiteral { integer: 3 },
                    },
                    TypeBridgePlanBulkCell {
                        present: 1,
                        value: TypeBridgePlanBulkLiteral { integer: 10 },
                    },
                ]
            })
            .collect::<Vec<_>>();
        // SAFETY: every frame/view remains live through this synchronous call.
        assert_eq!(
            unsafe {
                type_bridge_plan_builder_model_rows(
                    &mut builder,
                    thing,
                    columns.as_ptr(),
                    columns.len(),
                    cells.as_ptr(),
                    names.len(),
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(builder.error.is_none());
        assert_eq!(builder.fields.len(), 3);
        assert!(builder.values.is_empty());
    }

    #[test]
    fn present_null_text_is_a_sticky_value_fault_but_absence_never_reads_it() {
        for present in [0, 1] {
            let mut builder = TypeBridgePlanBuilder::new();
            let model = builder.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("item".into()));
            let field = builder.register_field(model, Some("item-id".into()));
            let thing = builder.thing(model, PlanBuilder::insert);
            let column = TypeBridgePlanBulkColumn {
                field,
                value_type: 1,
            };
            let cell = TypeBridgePlanBulkCell {
                present,
                value: TypeBridgePlanBulkLiteral {
                    text: TypeBridgeByteView {
                        data: ptr::null(),
                        length: 0,
                    },
                },
            };
            // SAFETY: absent union storage is ignored; NULL/zero text is a content fault when present.
            let status = unsafe {
                type_bridge_plan_builder_model_rows(&mut builder, thing, &column, 1, &cell, 1)
            };
            if present == 0 {
                assert_eq!(status, TypeBridgeStatus::Ok);
                assert!(builder.error.is_none());
            } else {
                assert_eq!(status, TypeBridgeStatus::InvalidArgument);
                assert_eq!(
                    builder.error.as_ref().unwrap().code().as_str(),
                    "c_plan_builder_value_invalid"
                );
            }
        }
    }

    #[test]
    fn bulk_frame_refuses_overflow_and_null_nested_views_before_recording() {
        let mut builder = TypeBridgePlanBuilder::new();
        let column = TypeBridgePlanBulkColumn {
            field: 0,
            value_type: 1,
        };
        let cell = TypeBridgePlanBulkCell {
            present: 1,
            value: TypeBridgePlanBulkLiteral {
                text: TypeBridgeByteView {
                    data: ptr::null(),
                    length: 1,
                },
            },
        };
        // SAFETY: null nested views and overflowing extents are rejected without dereferencing them.
        unsafe {
            assert_eq!(
                type_bridge_plan_builder_model_rows(&mut builder, 0, &column, 2, &cell, usize::MAX),
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(
                type_bridge_plan_builder_model_rows(&mut builder, 0, &column, 1, &cell, 1),
                TypeBridgeStatus::InvalidArgument
            );
        }
        assert!(builder.error.is_none());
        assert!(builder.values.is_empty());
    }
    #[repr(C)]
    struct HostRow {
        name: *const std::ffi::c_char,
        price: i64,
        stock: i64,
        has_stock: u8,
    }
    fn item_builder() -> (TypeBridgePlanBuilder, u32, [u32; 3]) {
        let mut builder = TypeBridgePlanBuilder::new();
        let model = builder.register_type(TYPE_BRIDGE_PLAN_THING_ENTITY, Some("item".into()));
        let fields = ["item-id", "price", "stock"]
            .map(|name| builder.register_field(model, Some(name.into())));
        let thing = builder.thing(model, PlanBuilder::insert);
        (builder, thing, fields)
    }
    fn host_columns(fields: [u32; 3]) -> [TypeBridgePlanBulkHostColumn; 3] {
        [
            TypeBridgePlanBulkHostColumn {
                field: fields[0],
                value_type: 1,
                value_offset: std::mem::offset_of!(HostRow, name),
                presence_offset: usize::MAX,
            },
            TypeBridgePlanBulkHostColumn {
                field: fields[1],
                value_type: 2,
                value_offset: std::mem::offset_of!(HostRow, price),
                presence_offset: usize::MAX,
            },
            TypeBridgePlanBulkHostColumn {
                field: fields[2],
                value_type: 2,
                value_offset: std::mem::offset_of!(HostRow, stock),
                presence_offset: std::mem::offset_of!(HostRow, has_stock),
            },
        ]
    }
    fn plans(
        builder: &TypeBridgePlanBuilder,
        fixture: &purchase_package::PurchasePackage,
    ) -> Vec<Vec<u8>> {
        builder
            .core
            .build_plans(
                fixture.authority.declared_schema(),
                fixture
                    .authority
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
                true,
                type_bridge_contract::split::SplitLimits::default(),
            )
            .unwrap()
            .plans()
            .iter()
            .map(|plan| plan.canonical_bytes().unwrap())
            .collect()
    }
    #[test]
    fn strided_ten_thousand_rows_equal_cell_frames_after_host_storage_is_freed() {
        let fixture = purchase_package::purchase_package();
        let (mut host_builder, host_thing, fields) = item_builder();
        let (mut cell_builder, cell_thing, _) = item_builder();
        let columns = host_columns(fields);
        let names = (0..10_000)
            .map(|index| std::ffi::CString::new(format!("I{index}")).unwrap())
            .collect::<Vec<_>>();
        let rows = names
            .iter()
            .enumerate()
            .map(|(index, name)| HostRow {
                name: name.as_ptr(),
                price: 3,
                stock: 10,
                has_stock: u8::from(index % 3 != 0),
            })
            .collect::<Vec<_>>();
        let cells = rows
            .iter()
            .zip(&names)
            .flat_map(|(row, name)| {
                [
                    TypeBridgePlanBulkCell {
                        present: 1,
                        value: TypeBridgePlanBulkLiteral {
                            text: TypeBridgeByteView {
                                data: name.as_ptr().cast(),
                                length: name.as_bytes().len(),
                            },
                        },
                    },
                    TypeBridgePlanBulkCell {
                        present: 1,
                        value: TypeBridgePlanBulkLiteral { integer: row.price },
                    },
                    TypeBridgePlanBulkCell {
                        present: row.has_stock,
                        value: TypeBridgePlanBulkLiteral { integer: row.stock },
                    },
                ]
            })
            .collect::<Vec<_>>();
        let cell_columns = columns.map(|column| TypeBridgePlanBulkColumn {
            field: column.field,
            value_type: column.value_type,
        });
        assert_eq!(cells.len() * size_of::<TypeBridgePlanBulkCell>(), 720_000);
        // SAFETY: both fully initialized input representations remain live during the synchronous calls.
        unsafe {
            assert_eq!(
                type_bridge_plan_builder_model_host_rows(
                    &mut host_builder,
                    host_thing,
                    columns.as_ptr(),
                    columns.len(),
                    rows.as_ptr().cast(),
                    rows.len(),
                    size_of::<HostRow>()
                ),
                TypeBridgeStatus::Ok
            );
            assert_eq!(
                type_bridge_plan_builder_model_rows(
                    &mut cell_builder,
                    cell_thing,
                    cell_columns.as_ptr(),
                    cell_columns.len(),
                    cells.as_ptr(),
                    rows.len()
                ),
                TypeBridgeStatus::Ok
            );
        }
        drop(rows);
        drop(cells);
        drop(names);
        assert!(host_builder.error.is_none());
        assert!(host_builder.values.is_empty());
        assert_eq!(host_builder.fields.len(), 3);
        assert_eq!(
            plans(&host_builder, &fixture),
            plans(&cell_builder, &fixture)
        );
    }
    #[test]
    fn strided_layout_refusals_leave_the_core_recording_unchanged() {
        let fixture = purchase_package::purchase_package();
        let row = HostRow {
            name: ptr::null(),
            price: 3,
            stock: 10,
            has_stock: 1,
        };
        for mode in 0..9 {
            let (mut builder, thing, fields) = item_builder();
            let before = plans(&builder, &fixture);
            let mut column = host_columns(fields)[1];
            let mut row_size = size_of::<HostRow>();
            let mut count = 1;
            let mut rows = (&row as *const HostRow).cast::<c_void>();
            match mode {
                0 => column.value_offset = usize::MAX,
                1 => column.value_offset = 1,
                2 => column.presence_offset = row_size,
                3 => row_size = 0,
                4 => row_size = 7,
                5 => count = usize::MAX,
                6 => rows = ptr::null(),
                7 => row_size -= 1,
                _ => rows = rows.wrapping_byte_add(1),
            }
            // SAFETY: every malformed extent is refused before any payload read.
            assert_eq!(
                unsafe {
                    type_bridge_plan_builder_model_host_rows(
                        &mut builder,
                        thing,
                        &column,
                        1,
                        rows,
                        count,
                        row_size,
                    )
                },
                TypeBridgeStatus::InvalidArgument
            );
            assert_eq!(
                builder.error.as_ref().unwrap().code().as_str(),
                "c_plan_bulk_layout_invalid"
            );
            assert_eq!(
                plans(&builder, &fixture),
                before,
                "invalid layout mode={mode}"
            );
        }
    }
    #[test]
    fn strided_absence_never_reads_a_string_payload_and_present_null_is_sticky() {
        for present in [0, 1] {
            let (mut builder, thing, fields) = item_builder();
            let column = TypeBridgePlanBulkHostColumn {
                presence_offset: std::mem::offset_of!(HostRow, has_stock),
                ..host_columns(fields)[0]
            };
            let row = HostRow {
                name: if present == 0 {
                    std::ptr::dangling()
                } else {
                    ptr::null()
                },
                price: 0,
                stock: 0,
                has_stock: present,
            };
            // SAFETY: absent string storage is never read; present NULL is a value refusal.
            let status = unsafe {
                type_bridge_plan_builder_model_host_rows(
                    &mut builder,
                    thing,
                    &column,
                    1,
                    (&row as *const HostRow).cast(),
                    1,
                    size_of::<HostRow>(),
                )
            };
            if present == 0 {
                assert_eq!(status, TypeBridgeStatus::Ok);
                assert!(builder.error.is_none());
            } else {
                assert_eq!(status, TypeBridgeStatus::InvalidArgument);
                assert_eq!(
                    builder.error.as_ref().unwrap().code().as_str(),
                    "c_plan_builder_value_invalid"
                );
            }
        }
    }
    #[test]
    fn strided_double_and_boolean_host_values_use_the_existing_scalar_parser() {
        #[repr(C)]
        struct NumericRow {
            price: f64,
            stock: u8,
        }
        let fixture = purchase_package::package_from_source(
            "format: typebridge.schema/v2\nattributes:\n  price: {value: double}\n  stock: {value: boolean}\nentities:\n  item:\n    owns: {price: {}, stock: {}}\n",
            "shop",
        );
        for (price, stock, valid) in [
            (1.25, 0, true),
            (1.25, 1, true),
            (1.25, 2, false),
            (f64::NAN, 1, false),
        ] {
            let (mut host, thing, fields) = item_builder();
            let (mut cells, cell_thing, _) = item_builder();
            let columns = [
                TypeBridgePlanBulkHostColumn {
                    field: fields[1],
                    value_type: 3,
                    value_offset: std::mem::offset_of!(NumericRow, price),
                    presence_offset: usize::MAX,
                },
                TypeBridgePlanBulkHostColumn {
                    field: fields[2],
                    value_type: 4,
                    value_offset: std::mem::offset_of!(NumericRow, stock),
                    presence_offset: usize::MAX,
                },
            ];
            let row = NumericRow { price, stock };
            let frame = [
                TypeBridgePlanBulkCell {
                    present: 1,
                    value: TypeBridgePlanBulkLiteral {
                        double_value: price,
                    },
                },
                TypeBridgePlanBulkCell {
                    present: 1,
                    value: TypeBridgePlanBulkLiteral { boolean: stock },
                },
            ];
            let cell_columns = columns.map(|column| TypeBridgePlanBulkColumn {
                field: column.field,
                value_type: column.value_type,
            });
            // SAFETY: initialized host and scalar frame lanes match their column metadata.
            let (host_status, cell_status) = unsafe {
                (
                    type_bridge_plan_builder_model_host_rows(
                        &mut host,
                        thing,
                        columns.as_ptr(),
                        columns.len(),
                        (&row as *const NumericRow).cast(),
                        1,
                        size_of::<NumericRow>(),
                    ),
                    type_bridge_plan_builder_model_rows(
                        &mut cells,
                        cell_thing,
                        cell_columns.as_ptr(),
                        cell_columns.len(),
                        frame.as_ptr(),
                        1,
                    ),
                )
            };
            assert_eq!(host_status, cell_status);
            if valid {
                assert_eq!(host_status, TypeBridgeStatus::Ok);
                assert_eq!(plans(&host, &fixture), plans(&cells, &fixture));
            } else {
                assert_eq!(host_status, TypeBridgeStatus::InvalidArgument);
                assert_eq!(
                    host.error.as_ref().unwrap().code().as_str(),
                    "c_plan_builder_value_invalid"
                );
            }
        }
    }
    #[test]
    fn empty_strided_bulk_never_reads_rows_and_builds_no_provider_plans() {
        let fixture = purchase_package::purchase_package();
        for rows in [ptr::null(), std::ptr::dangling::<u8>().cast::<c_void>()] {
            let (mut builder, thing, fields) = item_builder();
            let columns = host_columns(fields);
            // SAFETY: zero rows require no readable row storage, including no alignment requirement.
            assert_eq!(
                unsafe {
                    type_bridge_plan_builder_model_host_rows(
                        &mut builder,
                        thing,
                        columns.as_ptr(),
                        columns.len(),
                        rows,
                        0,
                        size_of::<HostRow>(),
                    )
                },
                TypeBridgeStatus::Ok
            );
            assert!(builder.error.is_none());
            assert!(plans(&builder, &fixture).is_empty());
        }
    }
}
