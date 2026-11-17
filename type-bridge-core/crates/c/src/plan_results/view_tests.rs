//! Actual C/C++ reads of native-owned layouts; no extra Rust loader dependency.
use super::*;
use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use type_bridge_contract::id::{AttributeId, TypeKind};
use type_bridge_contract::projection::c_view::{CViewKey, CViewLayouts};
use type_bridge_contract::temporal::{CanonicalDate, CanonicalDateTime, CanonicalDuration};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, DecimalValue};
use type_bridge_schema_codegen::CEmitter;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(file: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const std::ffi::c_char;
}
pub(super) struct Library {
    handle: *mut c_void,
    directory: PathBuf,
}
impl Drop for Library {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: this test owns the successfully opened library and every call ended.
            unsafe { dlclose(self.handle) };
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl Library {
    pub(super) fn compile(
        compiler: &str,
        language: &str,
        standard: &str,
        prefix: &str,
        header: &[u8],
        source: &str,
    ) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "c-plan-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(directory.join(prefix)).unwrap();
        let mut library = Self {
            handle: ptr::null_mut(),
            directory,
        };
        std::fs::write(library.directory.join(prefix).join("models.h"), header).unwrap();
        let input = library.directory.join("consumer.c");
        let output = library.directory.join("consumer.so");
        std::fs::write(&input, source).unwrap();
        let result = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-shared",
                "-fPIC",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
            ])
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("include"))
            .arg("-I")
            .arg(&library.directory)
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = CString::new(output.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: the freshly compiled library only reads matching immutable C views.
        library.handle = unsafe { dlopen(output.as_ptr(), 2) };
        if library.handle.is_null() {
            // SAFETY: dlerror returns the loader's live NUL-terminated error string.
            let error = unsafe { CStr::from_ptr(dlerror()) }.to_string_lossy();
            panic!("dlopen: {error}");
        }
        library
    }
    pub(super) fn inspect(&self, data: *const u8, mode: i32) {
        // SAFETY: the named symbol has the exact C signature in the source below.
        let pointer = unsafe { dlsym(self.handle, c"inspect_view".as_ptr()) };
        assert!(!pointer.is_null());
        // SAFETY: POSIX dlsym's function address names the matching exported inspector.
        let inspect: unsafe extern "C" fn(*const c_void, i32) -> i32 =
            unsafe { std::mem::transmute(pointer) };
        // SAFETY: the owning native model/string/array storage stays alive throughout.
        assert_eq!(
            unsafe { inspect(data.cast(), mode) },
            0,
            "compiled consumer mode {mode}"
        );
    }
}
const SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  id: { value: string }
  l: { value: integer }
  d: { value: double }
  b: { value: boolean }
  s: { value: string }
  day: { value: date }
  clock: { value: datetime }
  zone: { value: datetime-tz }
  dec: { value: decimal }
  dur: { value: duration }
  tags: { value: string }
  note: { value: string }
entities:
  actor:
    abstract: true
    owns:
      id: { key: true }
  sample:
    sub: actor
    owns:
      l: { card: 1 }
      d: { card: 1 }
      b: { card: 1 }
      s: { card: 1 }
      day: { card: 1 }
      clock: { card: 1 }
      zone: { card: 1 }
      dec: { card: 1 }
      dur: { card: 1 }
      tags: { card: { min: 0, max: 3 } }
      note: { card: { min: 0, max: 1 } }
relations:
  link:
    relates:
      member: { card: { min: 0, max: 3 } }
plays:
  actor:
    link: [member]
"#;

#[test]
fn c_and_cpp_read_every_scalar_polymorphic_nested_and_empty_role_view() {
    let package = crate::entity_crud::tests::package_from_source("cv", SOURCE);
    let state = package.state();
    let layouts = CViewLayouts::for_projection(&state._projection).unwrap();
    let sample = TypeId::new(TypeKind::Entity, "sample").unwrap();
    let actor = TypeId::new(TypeKind::Entity, "actor").unwrap();
    let link = TypeId::new(TypeKind::Relation, "link").unwrap();
    let member = RoleId::new("link", "member").unwrap();
    let values = [
        (
            "id",
            CanonicalValue::String(CanonicalString::new("sample").unwrap()),
        ),
        ("l", CanonicalValue::Long(7)),
        (
            "d",
            CanonicalValue::Double(CanonicalDouble::new(1.25).unwrap()),
        ),
        ("b", CanonicalValue::Boolean(true)),
        (
            "s",
            CanonicalValue::String(CanonicalString::new("hello").unwrap()),
        ),
        (
            "day",
            CanonicalValue::Date(CanonicalDate::from_str("2024-01-02").unwrap()),
        ),
        (
            "clock",
            CanonicalValue::DateTime(CanonicalDateTime::from_str("2024-01-02T03:04:05").unwrap()),
        ),
        (
            "zone",
            CanonicalValue::DateTimeTz(
                type_bridge_schema::parse_provider_datetime_tz("2024-01-02T03:04:05Z").unwrap(),
            ),
        ),
        (
            "dec",
            CanonicalValue::Decimal(DecimalValue::new("1.25").unwrap()),
        ),
        (
            "dur",
            CanonicalValue::Duration(CanonicalDuration::from_str("P1D").unwrap()),
        ),
    ];
    let model = &state._projection.models()[&sample];
    let mut fields = values
        .into_iter()
        .map(|(name, value)| {
            let field = model
                .query_tokens()
                .fields()
                .keys()
                .find(|field| field.attribute().label().as_str() == name)
                .unwrap()
                .clone();
            let attribute = TypeId::new(TypeKind::Attribute, name).unwrap();
            (
                field,
                vec![
                    ProjectedAttributeValue::try_new(&state.installed_projection, attribute, value)
                        .unwrap(),
                ],
            )
        })
        .collect::<Vec<_>>();
    let tags = model
        .query_tokens()
        .fields()
        .keys()
        .find(|field| field.attribute() == &AttributeId::new("tags").unwrap())
        .unwrap()
        .clone();
    fields.push((
        tags,
        ["first", "second"]
            .into_iter()
            .map(|text| {
                ProjectedAttributeValue::try_new(
                    &state.installed_projection,
                    TypeId::new(TypeKind::Attribute, "tags").unwrap(),
                    CanonicalValue::String(CanonicalString::new(text).unwrap()),
                )
                .unwrap()
            })
            .collect(),
    ));
    let thing = ProjectedThing::try_new(
        &state.installed_projection,
        sample.clone(),
        "0x01".to_owned(),
        fields.clone(),
        vec![],
    )
    .unwrap();
    let role = &state._projection.models()[&link].complete_read().roles()[&member];
    let player = ProjectedRolePlayer::try_new_complete_for_hydration(
        &state.installed_projection,
        role,
        thing.try_to_reference(&state.installed_projection).unwrap(),
        fields,
    )
    .unwrap();
    let relation = ProjectedThing::try_new(
        &state.installed_projection,
        link.clone(),
        "0x02".to_owned(),
        vec![],
        vec![(member.clone(), vec![player])],
    )
    .unwrap();
    let empty = ProjectedThing::try_new(
        &state.installed_projection,
        link.clone(),
        "0x03".to_owned(),
        vec![],
        vec![(member.clone(), vec![])],
    )
    .unwrap();
    let emitted = CEmitter::new()
        .emit(&state._projection, &state._authority)
        .unwrap();
    let header = emitted.get("include/cv/models.h").unwrap();
    let mut role_layout = layouts
        .get(&CViewKey::Role {
            owner: link.clone(),
            role: member,
        })
        .unwrap();
    while let Some(alias) = &role_layout.alias {
        role_layout = layouts.get(alias).unwrap();
    }
    let player = if role_layout.union_offset.is_some() {
        "view->member[0].value.sample"
    } else {
        "view->member[0]"
    };
    let source = format!(
        r#"
#include <cv/models.h>
static int check_sample(const cv_sample_plan_exact_view_t *view) {{
  if (strcmp(view->iid, "0x01") || strcmp(view->id, "sample")) return 1;
  if (view->deleted || view->l != 7 || view->d != 1.25 || !view->b) return 2;
  if (strcmp(view->s, "hello") || strcmp(view->day, "2024-01-02")) return 3;
  if (strcmp(view->clock, "2024-01-02T03:04:05") || strcmp(view->zzone, "2024-01-02T03:04:05Z")) return 4;
  if (strcmp(view->dec, "1.25") || strcmp(view->dur, "P1D")) return 5;
  if (view->tags_count != 2u || strcmp(view->tags[0], "first") || strcmp(view->tags[1], "second")) return 6;
  if (view->has_note || view->note != NULL) return 7;
  return 0;
}}
#ifdef __cplusplus
extern "C"
#endif
int inspect_view(const void *bytes, int mode) {{
  if (mode == 0) return check_sample((const cv_sample_plan_view_t *)bytes);
  if (mode == 1) {{ const cv_actor_plan_view_t *view = (const cv_actor_plan_view_t *)bytes; if (view->kind != view->value.sample.kind) return 8; return check_sample(&view->value.sample); }}
  {{ const cv_link_plan_view_t *view = (const cv_link_plan_view_t *)bytes;
    if (mode == 3) return view->member_count == 0u && view->member == NULL ? 0 : 9;
    if (view->member_count != 1u || view->member == NULL) return 10;
    if ({player}.deleted || strcmp({player}.iid, "0x01") || strcmp({player}.id, "sample")) return 11;
    if (!{player}.has_l || {player}.l != 7 || !{player}.has_s || strcmp({player}.s, "hello")) return 12;
  }}
  return 0;
}}
"#
    );
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let library = Library::compile(compiler, language, standard, "cv", header, &source);
        for (mode, declared, thing) in [
            (0, &sample, &thing),
            (1, &actor, &thing),
            (2, &link, &relation),
            (3, &link, &empty),
        ] {
            let models = vec![vec![ModelViewData::root(state, thing).unwrap()]];
            let storage = views::ViewStorage::build(&layouts, declared, &models).unwrap();
            library.inspect(storage.row(0).unwrap().as_ptr(), mode);
        }
    }
}
