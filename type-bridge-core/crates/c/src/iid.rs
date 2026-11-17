//! Validated, owned TypeDB identity values shared by C query and CRUD APIs.

use std::{mem::size_of, ptr};

use type_bridge_contract::id::Iid;

use crate::abi::{TypeBridgeByteView, TypeBridgeStatus, close_box, guarded};
use crate::allocation::{AllocationSite, ReservedBox};
use crate::generated_preflight::direct_output_preflight;

/// Opaque validated identity. Inline storage makes its entire borrow one range.
pub struct TypeBridgeIid {
    bytes: [u8; 258],
    length: usize,
}

impl TypeBridgeIid {
    pub(crate) fn new(text: &str) -> Result<Self, TypeBridgeStatus> {
        let value = Iid::new(text).map_err(|_| TypeBridgeStatus::InvalidArgument)?;
        let mut bytes = [0; 258];
        bytes[..text.len()].copy_from_slice(value.as_str().as_bytes());
        Ok(Self {
            bytes,
            length: text.len(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.length]).expect("validated ASCII IID")
    }
}

/// Construct an owned IID from `0x` followed by one to 256 hexadecimal digits.
/// Spelling and case are preserved; invalid input is rejected before any I/O.
/// # Safety
/// Text is readable; output is writable and disjoint from the input bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_iid_open(
    text: TypeBridgeByteView,
    out: *mut *mut TypeBridgeIid,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight =
            match direct_output_preflight(&[(out.cast(), size_of::<*mut TypeBridgeIid>())]) {
                Ok(value) => value,
                Err(status) => return status,
            };
        if let Err(status) = preflight.check_bytes(text.data.cast(), text.length) {
            return status;
        }
        if out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: output and input ranges are disjoint.
        unsafe {
            out.write_unaligned(ptr::null_mut());
        }
        if text.data.is_null() || !(3..=258).contains(&text.length) {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains this bounded immutable byte range.
        let text = match std::str::from_utf8(unsafe {
            std::slice::from_raw_parts(text.data, text.length)
        }) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::InvalidArgument,
        };
        let value = match TypeBridgeIid::new(text) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let reservation = match ReservedBox::try_new(AllocationSite::IidHandle) {
            Ok(value) => value,
            Err(_) => return TypeBridgeStatus::ResourceLimit,
        };
        // SAFETY: output was fenced and initialized.
        unsafe {
            out.write_unaligned(Box::into_raw(reservation.initialize(value)));
        }
        TypeBridgeStatus::Ok
    })
}

/// Borrow the validated spelling until the IID is closed.
/// # Safety
/// IID is live; output is writable and disjoint from the entire IID handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_iid_text(
    iid: *const TypeBridgeIid,
    out: *mut TypeBridgeByteView,
) -> TypeBridgeStatus {
    guarded(|| {
        let preflight =
            match direct_output_preflight(&[(out.cast(), size_of::<TypeBridgeByteView>())]) {
                Ok(value) => value,
                Err(status) => return status,
            };
        if !iid.is_null()
            && let Err(status) = preflight.check_bytes(iid.cast(), size_of::<TypeBridgeIid>())
        {
            return status;
        }
        if out.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: output is disjoint from the retained handle and inline text.
        unsafe {
            out.write_unaligned(TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            });
        }
        if iid.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the complete immutable IID.
        let text = unsafe { &*iid }.as_str();
        // SAFETY: preflight fenced the entire retained IID.
        unsafe {
            out.write_unaligned(TypeBridgeByteView {
                data: text.as_ptr(),
                length: text.len(),
            });
        }
        TypeBridgeStatus::Ok
    })
}

/// Free a validated identity and clear its owner; repeated close is safe.
/// # Safety
/// Caller uniquely owns a writable pointer slot containing a live IID or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_iid_close(iid: *mut *mut TypeBridgeIid) -> TypeBridgeStatus {
    // SAFETY: forwarded ownership contract.
    unsafe { close_box(iid) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_crud::tests::byte_view;

    #[test]
    fn construction_validates_and_preserves_spelling() {
        for text in [
            "0x0".to_string(),
            "0xAbC09".into(),
            format!("0x{}", "f".repeat(256)),
        ] {
            let mut iid = ptr::null_mut();
            let mut view = byte_view(b"");
            // SAFETY: live disjoint input/output storage, uniquely owned handle.
            unsafe {
                assert_eq!(
                    type_bridge_iid_open(byte_view(text.as_bytes()), &mut iid),
                    TypeBridgeStatus::Ok
                );
                assert_eq!(type_bridge_iid_text(iid, &mut view), TypeBridgeStatus::Ok);
                assert_eq!(
                    std::slice::from_raw_parts(view.data, view.length),
                    text.as_bytes()
                );
                assert_eq!(
                    type_bridge_iid_text(iid, view.data.cast_mut().cast()),
                    TypeBridgeStatus::InvalidArgument
                );
                assert_eq!(type_bridge_iid_close(&mut iid), TypeBridgeStatus::Ok);
                assert!(iid.is_null());
                assert_eq!(type_bridge_iid_close(&mut iid), TypeBridgeStatus::Ok);
            }
        }
        for text in [
            "".to_string(),
            "0x".into(),
            "0X1".into(),
            "0xg".into(),
            "0x1\0".into(),
            format!("0x{}", "1".repeat(257)),
        ] {
            let mut iid = ptr::dangling_mut();
            // SAFETY: live disjoint text and output slot; invalid input clears output.
            assert_eq!(
                unsafe { type_bridge_iid_open(byte_view(text.as_bytes()), &mut iid) },
                TypeBridgeStatus::InvalidArgument
            );
            assert!(iid.is_null());
        }
    }
}
