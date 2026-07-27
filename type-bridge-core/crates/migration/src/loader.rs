//! Native Rust sidecar loader for migration files.
//!
//! Reads `NNNN_<name>.json` sidecar files that the generator writes beside
//! the corresponding `NNNN_<name>.py` source files.  The sidecar carries the
//! serde [`MigrationSpec`] produced from the same op list as the `.py` — so
//! the Rust CLI can hydrate a [`MigrationGraph`] without importing Python.
//!
//! The loader is pure `std::fs` + `serde_json`; it opens no TypeDB
//! transaction and has no dependency on `type_bridge_orm` (invariant 7).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::checksum::migration_file_checksum;
use crate::error::{MigrationError, Result};
use crate::spec::{MigrationGraph, MigrationSpec};

/// Load the sidecar spec for a given `.py` migration path.
///
/// Derives the sidecar path by replacing the `.py` extension with `.json`
/// (same stem, sibling file).
///
/// - If the `.json` sibling **does not exist** → `Ok(None)`.  The caller
///   should fall back to the trusted-import Python path for this file.
/// - If it **exists** → read, deserialize, and return `Ok(Some(spec))`.
/// - If it exists but is malformed → `Err(MigrationError::Loader { .. })`.
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when the sidecar exists but cannot be
/// read or deserialized.
pub fn load_sidecar(py_path: &Path) -> Result<Option<MigrationSpec>> {
    let json_path = py_path.with_extension("json");
    if !json_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&json_path).map_err(|err| MigrationError::Loader {
        message: format!("failed to read sidecar {}: {err}", json_path.display()),
    })?;
    let spec: MigrationSpec =
        serde_json::from_str(&content).map_err(|err| MigrationError::Loader {
            message: format!("failed to parse sidecar {}: {err}", json_path.display()),
        })?;
    Ok(Some(spec))
}

/// Walk `dir` and load all `NNNN_*.json` sidecar files into a sorted
/// [`MigrationGraph`].
///
/// Only files whose stems match the four-digit prefix pattern
/// (`[0-9][0-9][0-9][0-9]_*`) and whose extension is `.json` are loaded.
/// `.py` files and any other non-matching files are skipped.  The resulting
/// [`MigrationGraph`] is sorted by file stem (lexicographic / discovery
/// order), which matches Python's `discover()` sort.
///
/// This is the dir-native loader consumed by the Rust CLI (sub-plan 08).
/// It does not invoke Python, does not `exec_module`, and opens no
/// transaction.
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when the directory cannot be read or
/// a matching sidecar file cannot be read or deserialized.
pub fn load_dir(dir: &Path) -> Result<MigrationGraph> {
    let read_dir = std::fs::read_dir(dir).map_err(|err| MigrationError::Loader {
        message: format!("failed to read migrations dir {}: {err}", dir.display()),
    })?;

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|err| MigrationError::Loader {
            message: format!("failed to iterate migrations dir {}: {err}", dir.display()),
        })?;
        let path = entry.path();

        let file_name = path.file_name().and_then(|name| name.to_str());
        // Adoption archives are deliberately not executable sidecars.
        if file_name.is_some_and(|name| name.ends_with(".adoption.json")) {
            continue;
        }
        // Only consider `.json` files.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        // The stem must match `NNNN_*` (four digits then underscore).
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };

        if !is_migration_stem(&stem) {
            continue;
        }

        entries.push((stem, path));
    }

    // Sort by stem for stable discovery order.
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut migrations = Vec::with_capacity(entries.len());
    for (stem, path) in entries {
        let content = std::fs::read_to_string(&path).map_err(|err| MigrationError::Loader {
            message: format!("failed to read sidecar {}: {err}", path.display()),
        })?;
        let spec: MigrationSpec =
            serde_json::from_str(&content).map_err(|err| MigrationError::Loader {
                message: format!(
                    "failed to parse sidecar {} (stem={stem}): {err}",
                    path.display()
                ),
            })?;
        migrations.push(spec);
    }

    Ok(MigrationGraph { migrations })
}

/// Walk `dir` and load all `NNNN_*.json` sidecar files into a sorted
/// [`MigrationGraph`], then validate that each sidecar's embedded source
/// identity agrees with the current `.py` file.
///
/// This is the **checked** variant of [`load_dir`], intended for use by the
/// Rust CLI whenever it is the execution source.  The check guards against
/// sidecar drift: if a developer hand-edits the `.py` after the sidecar was
/// generated, its full `source_sha256` (new sidecars) or legacy shortened
/// text `checksum` will disagree, and this function returns an error rather
/// than silently executing a stale sidecar.
///
/// # Invariant
///
/// `source_sha256`, when present, is authoritative and binds exact raw bytes.
/// Sidecars without it retain the released UTF-8/universal-newline checksum
/// behavior. If the `.py` file is absent for a given sidecar the check is
/// skipped (legacy sidecar-only migration; no `.py` to compare).
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when:
/// - The directory or a sidecar cannot be read (same as [`load_dir`]).
/// - A sidecar's source digest or legacy checksum disagrees with the `.py`
///   source — "sidecar drift: regenerate the migration" (D6 guard).
pub fn load_dir_checked(dir: &Path) -> Result<MigrationGraph> {
    let read_dir = std::fs::read_dir(dir).map_err(|err| MigrationError::Loader {
        message: format!("failed to read migrations dir {}: {err}", dir.display()),
    })?;

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    let mut python_stems = std::collections::BTreeSet::new();
    for entry in read_dir {
        let entry = entry.map_err(|err| MigrationError::Loader {
            message: format!("failed to iterate migrations dir {}: {err}", dir.display()),
        })?;
        let path = entry.path();

        let file_name = path.file_name().and_then(|name| name.to_str());
        if file_name.is_some_and(|name| name.ends_with(".adoption.json")) {
            continue;
        }
        let extension = path.extension().and_then(|e| e.to_str());
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };
        if !is_migration_stem(&stem) {
            continue;
        }
        match extension {
            Some("json") => entries.push((stem, path)),
            Some("py") => {
                python_stems.insert(stem);
            }
            _ => {}
        }
    }

    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    // A migration-shaped `.py` with no sidecar is a Python-only migration
    // the released Python loader would import dynamically. This is the one
    // deliberate fail-closed exception to V1 sidecar-loader parity: the
    // native loader cannot execute it, and silently omitting it would
    // truncate the history, so require the explicit adoption conversion.
    let sidecar_stems: std::collections::BTreeSet<&str> =
        entries.iter().map(|(stem, _)| stem.as_str()).collect();
    let orphans: Vec<String> = python_stems
        .iter()
        .filter(|stem| !sidecar_stems.contains(stem.as_str()))
        .cloned()
        .collect();
    if !orphans.is_empty() {
        return Err(MigrationError::Loader {
            message: format!(
                "Python-only migrations without JSON sidecars in {}: {}. \
                 The native loader cannot execute a dynamically imported \
                 migration; each listed file needs a JSON sidecar recording \
                 its checked execution spec before the native migration \
                 path can use this history. Generate the sidecars with \
                 `python -m type_bridge.migration.sidecar <migrations-dir>`.",
                dir.display(),
                orphans.join(", ")
            ),
        });
    }

    let mut migrations = Vec::with_capacity(entries.len());
    for (stem, json_path) in entries {
        let content =
            std::fs::read_to_string(&json_path).map_err(|err| MigrationError::Loader {
                message: format!("failed to read sidecar {}: {err}", json_path.display()),
            })?;
        let spec: MigrationSpec =
            serde_json::from_str(&content).map_err(|err| MigrationError::Loader {
                message: format!(
                    "failed to parse sidecar {} (stem={stem}): {err}",
                    json_path.display()
                ),
            })?;

        let py_path = json_path.with_extension("py");
        let has_python_source = python_stems.contains(&stem);

        // New sidecars bind the exact source bytes without interpreting them
        // as text. This full digest is locale- and newline-independent, and
        // supersedes the shorter legacy text checksum when present so PEP 263
        // sources remain verifiable without reimplementing Python's codecs.
        if let Some(sidecar_sha256) = &spec.source_sha256 {
            if !is_lower_hex_sha256(sidecar_sha256) {
                return Err(MigrationError::Loader {
                    message: format!(
                        "sidecar for {stem} carries a malformed source_sha256; \
                         expected exactly 64 lowercase hexadecimal characters"
                    ),
                });
            }
            if has_python_source {
                let raw = read_python_bytes(&py_path)?;
                let computed = format!("{:x}", Sha256::digest(&raw));
                if computed != *sidecar_sha256 {
                    return Err(MigrationError::Loader {
                        message: format!(
                            "sidecar drift detected for {stem}: the raw .py file has been \
                             modified since the sidecar was generated \
                             (sidecar source_sha256={sidecar_sha256}, \
                             current .py source_sha256={computed}). \
                             Regenerate the migration to sync the sidecar."
                        ),
                    });
                }
            }
        } else if let Some(sidecar_checksum) = &spec.checksum {
            // D6 legacy drift guard: sidecars without a raw digest retain the
            // released read/decode/universal-newline behavior exactly.
            if has_python_source {
                let py_text = read_python_text(&py_path)?;
                let computed = migration_file_checksum(&py_text);
                if computed != *sidecar_checksum {
                    return Err(MigrationError::Loader {
                        message: format!(
                            "sidecar drift detected for {stem}: the .py file has been \
                             modified since the sidecar was generated \
                             (sidecar checksum={sidecar_checksum}, \
                             current .py checksum={computed}). \
                             Regenerate the migration to sync the sidecar."
                        ),
                    });
                }
            }
        }

        migrations.push(spec);
    }

    Ok(MigrationGraph { migrations })
}

/// Read a legacy `.py` migration exactly as the released Python loader
/// does: UTF-8 decode plus universal-newline translation, so a CRLF
/// checkout hashes to the same checksum `Path.read_text()` produced when
/// the sidecar or ledger was written.
fn read_python_text(py_path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(py_path).map_err(|err| MigrationError::Loader {
        message: format!(
            "failed to read .py for drift check {}: {err}",
            py_path.display()
        ),
    })?;
    if !raw.contains('\r') {
        return Ok(raw);
    }
    Ok(raw.replace("\r\n", "\n").replace('\r', "\n"))
}

fn read_python_bytes(py_path: &Path) -> Result<Vec<u8>> {
    std::fs::read(py_path).map_err(|err| MigrationError::Loader {
        message: format!(
            "failed to read .py for raw-source drift check {}: {err}",
            py_path.display()
        ),
    })
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Open an adoption artifact without following its final path component.
///
/// The released V1 loaders above intentionally retain their historical
/// symlink-following behavior. Only the new bounded adoption reader calls
/// this helper.
pub(crate) fn open_regular_readonly_nofollow(
    directory: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
) -> std::io::Result<cap_std::fs::File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};

    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = directory.open_with(name, &options)?;

    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "migration artifact is not a regular file",
        ));
    }
    Ok(file)
}

/// Return `true` if the stem matches the migration naming convention:
/// four ASCII digits followed by an underscore.
fn is_migration_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    if bytes.len() < 5 {
        return false;
    }
    bytes[..4].iter().all(|b| b.is_ascii_digit()) && bytes[4] == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{MigrationSpec, OperationSpec};

    /// Build a minimal but valid `MigrationSpec` for testing.
    fn make_spec(name: &str) -> MigrationSpec {
        MigrationSpec {
            app_label: "test_app".to_string(),
            name: name.to_string(),
            dependencies: vec![],
            operations: vec![OperationSpec::RunTypeql {
                forward: format!("define attribute {name}, value string;"),
                reverse: None,
            }],
            checksum: Some("abc123".to_string()),
            source_sha256: None,
            reversible: false,
        }
    }

    /// Write a `MigrationSpec` to a `.json` file in `dir` using the given stem.
    fn write_sidecar(dir: &Path, stem: &str, spec: &MigrationSpec) {
        let json = serde_json::to_string(spec).unwrap();
        std::fs::write(dir.join(format!("{stem}.json")), json).unwrap();
    }

    /// Write an empty `.py` file in `dir` using the given stem.
    fn write_py(dir: &Path, stem: &str) {
        std::fs::write(dir.join(format!("{stem}.py")), b"class Migration: pass\n").unwrap();
    }

    // ── load_sidecar ──────────────────────────────────────────────────────────

    #[test]
    fn load_sidecar_returns_some_for_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = make_spec("0001_initial");
        write_sidecar(tmp.path(), "0001_initial", &spec);
        write_py(tmp.path(), "0001_initial");

        let py_path = tmp.path().join("0001_initial.py");
        let result = load_sidecar(&py_path).unwrap();

        assert_eq!(result, Some(spec));
    }

    #[test]
    fn load_sidecar_returns_none_when_no_json_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        write_py(tmp.path(), "0001_initial");

        let py_path = tmp.path().join("0001_initial.py");
        let result = load_sidecar(&py_path).unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn load_sidecar_returns_error_on_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("0001_initial.json"), b"{ not valid json }").unwrap();
        write_py(tmp.path(), "0001_initial");

        let py_path = tmp.path().join("0001_initial.py");
        let result = load_sidecar(&py_path);

        assert!(result.is_err(), "expected Err on malformed JSON");
        let err = result.unwrap_err();
        assert!(
            matches!(err, MigrationError::Loader { .. }),
            "expected MigrationError::Loader, got: {err:?}"
        );
    }

    // ── load_dir ─────────────────────────────────────────────────────────────

    #[test]
    fn load_dir_loads_sidecars_and_skips_bare_py() {
        let tmp = tempfile::tempdir().unwrap();

        // Two sidecar-bearing migrations.
        let spec1 = make_spec("0001_initial");
        let spec2 = make_spec("0002_add_attr");
        write_sidecar(tmp.path(), "0001_initial", &spec1);
        write_py(tmp.path(), "0001_initial");
        write_sidecar(tmp.path(), "0002_add_attr", &spec2);
        write_py(tmp.path(), "0002_add_attr");

        // One legacy .py with NO sidecar — must be skipped by load_dir.
        write_py(tmp.path(), "0003_legacy");

        let graph = load_dir(tmp.path()).unwrap();

        assert_eq!(
            graph.migrations.len(),
            2,
            "expected exactly two specs from the two sidecars"
        );
        assert_eq!(
            graph.migrations[0], spec1,
            "first spec should be 0001_initial"
        );
        assert_eq!(
            graph.migrations[1], spec2,
            "second spec should be 0002_add_attr"
        );
    }

    #[test]
    fn load_dir_sorts_by_stem() {
        let tmp = tempfile::tempdir().unwrap();

        // Write in reverse order to verify sort is applied.
        let spec2 = make_spec("0002_b");
        let spec1 = make_spec("0001_a");
        write_sidecar(tmp.path(), "0002_b", &spec2);
        write_sidecar(tmp.path(), "0001_a", &spec1);

        let graph = load_dir(tmp.path()).unwrap();

        assert_eq!(graph.migrations.len(), 2);
        assert_eq!(graph.migrations[0].name, "0001_a");
        assert_eq!(graph.migrations[1].name, "0002_b");
    }

    #[test]
    fn sidecar_loaders_recognize_minimal_released_migration_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let mut spec = make_spec("0001_");
        spec.checksum = None;
        write_sidecar(tmp.path(), "0001_", &spec);
        write_py(tmp.path(), "0001_");

        assert_eq!(load_dir(tmp.path()).unwrap().migrations, vec![spec.clone()]);
        assert_eq!(load_dir_checked(tmp.path()).unwrap().migrations, vec![spec]);
    }

    #[test]
    fn load_dir_integration_smoke_sidecar_and_no_sidecar() {
        // Integration smoke: dir with one sidecar-bearing .py+.json pair and one
        // legacy .py-only file; load_dir returns MigrationGraph with exactly the
        // one sidecar spec, confirming prefer-sidecar / fall-back-to-None seam
        // for the pure-Rust (CLI) consumption path.
        let tmp = tempfile::tempdir().unwrap();

        let spec = make_spec("0001_initial");
        write_sidecar(tmp.path(), "0001_initial", &spec);
        write_py(tmp.path(), "0001_initial");

        // Legacy: .py only, no sidecar.
        write_py(tmp.path(), "0002_legacy");

        let graph = load_dir(tmp.path()).unwrap();

        assert_eq!(
            graph.migrations.len(),
            1,
            "load_dir must load only JSON sidecars; the bare .py must not appear"
        );
        assert_eq!(graph.migrations[0], spec);
    }

    // ── load_dir_checked (D6 drift guard) ────────────────────────────────────

    /// Build a `MigrationSpec` whose `checksum` is computed over a real `.py`
    /// text body using `migration_file_checksum`, so the drift guard accepts it.
    fn make_spec_with_real_checksum(name: &str, py_text: &str) -> MigrationSpec {
        use crate::checksum::migration_file_checksum;
        MigrationSpec {
            app_label: "test_app".to_string(),
            name: name.to_string(),
            dependencies: vec![],
            operations: vec![OperationSpec::RunTypeql {
                forward: format!("define attribute {name}, value string;"),
                reverse: None,
            }],
            checksum: Some(migration_file_checksum(py_text)),
            source_sha256: None,
            reversible: false,
        }
    }

    #[test]
    fn load_dir_checked_accepts_matching_checksum() {
        // A sidecar whose embedded checksum was computed from the same .py text
        // that is on disk must be accepted by the drift guard.
        let tmp = tempfile::tempdir().unwrap();
        let py_text = "class Migration: pass\n";

        let spec = make_spec_with_real_checksum("0001_initial", py_text);
        write_sidecar(tmp.path(), "0001_initial", &spec);
        std::fs::write(tmp.path().join("0001_initial.py"), py_text.as_bytes()).unwrap();

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(graph.migrations.len(), 1);
        assert_eq!(graph.migrations[0].name, "0001_initial");
    }

    #[test]
    fn load_dir_checked_accepts_crlf_checked_out_python() {
        // The released Python loader hashes after Path.read_text()'s
        // universal-newline translation. A CRLF checkout of the same
        // logical source must therefore verify against a sidecar written
        // from the LF form.
        let tmp = tempfile::tempdir().unwrap();
        let lf_text = "class Migration: pass\n# checked out with CRLF\n";
        let crlf_text = lf_text.replace('\n', "\r\n");

        let spec = make_spec_with_real_checksum("0001_initial", lf_text);
        write_sidecar(tmp.path(), "0001_initial", &spec);
        std::fs::write(tmp.path().join("0001_initial.py"), crlf_text.as_bytes()).unwrap();

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(graph.migrations.len(), 1);
    }

    #[test]
    fn load_dir_checked_raw_digest_supersedes_legacy_text_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let py_bytes = b"# -*- coding: cp1252 -*-\nlabel = '\xe9'\n";
        let mut spec = make_spec("0001_encoded");
        // This deliberately cannot describe the source below: a checked raw
        // digest is authoritative when both fields are present.
        spec.checksum = Some("0000000000000000".to_string());
        spec.source_sha256 = Some(format!("{:x}", Sha256::digest(py_bytes)));
        write_sidecar(tmp.path(), "0001_encoded", &spec);
        std::fs::write(tmp.path().join("0001_encoded.py"), py_bytes).unwrap();

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(graph.migrations, vec![spec]);
    }

    #[test]
    fn load_dir_checked_rejects_raw_source_drift_before_legacy_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let original = b"class Migration: pass\n";
        let current = b"class Migration: pass\n# changed\n";
        let mut spec =
            make_spec_with_real_checksum("0001_raw_drift", std::str::from_utf8(current).unwrap());
        spec.source_sha256 = Some(format!("{:x}", Sha256::digest(original)));
        write_sidecar(tmp.path(), "0001_raw_drift", &spec);
        std::fs::write(tmp.path().join("0001_raw_drift.py"), current).unwrap();

        let error = load_dir_checked(tmp.path()).unwrap_err().to_string();
        assert!(error.contains("source_sha256"), "unexpected error: {error}");
    }

    #[test]
    fn load_dir_checked_rejects_malformed_raw_source_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut spec = make_spec("0001_malformed");
        spec.source_sha256 = Some("ABC".to_string());
        write_sidecar(tmp.path(), "0001_malformed", &spec);

        let error = load_dir_checked(tmp.path()).unwrap_err().to_string();
        assert!(
            error.contains("malformed source_sha256"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_dir_checked_rejects_python_only_migrations() {
        // A migration-shaped .py with no sidecar is a dynamically imported
        // Python-only migration; omitting it would truncate the history,
        // so the checked loader must fail with a conversion requirement.
        let tmp = tempfile::tempdir().unwrap();
        let py_text = "class Migration: pass\n";
        let spec = make_spec_with_real_checksum("0001_initial", py_text);
        write_sidecar(tmp.path(), "0001_initial", &spec);
        std::fs::write(tmp.path().join("0001_initial.py"), py_text.as_bytes()).unwrap();
        write_py(tmp.path(), "0002_custom");

        let err = load_dir_checked(tmp.path()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("0002_custom") && message.contains("sidecar"),
            "error must name the orphan and the sidecar requirement; got: {message}"
        );
    }

    #[test]
    fn load_dir_checked_rejects_stale_sidecar() {
        // If the .py is hand-edited AFTER the sidecar was generated, the
        // embedded checksum will disagree with the current .py text.  The
        // drift guard must reject the sidecar rather than silently executing it.
        let tmp = tempfile::tempdir().unwrap();
        let original_py_text = "class Migration: pass\n";
        let mutated_py_text = "class Migration: pass\n# hand-edited after sidecar generation\n";

        // Sidecar checksum reflects the ORIGINAL .py text.
        let spec = make_spec_with_real_checksum("0001_initial", original_py_text);
        write_sidecar(tmp.path(), "0001_initial", &spec);

        // Write the MUTATED .py text to disk — the sidecar is now stale.
        std::fs::write(
            tmp.path().join("0001_initial.py"),
            mutated_py_text.as_bytes(),
        )
        .unwrap();

        let result = load_dir_checked(tmp.path());
        assert!(
            result.is_err(),
            "load_dir_checked must reject a stale sidecar"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, MigrationError::Loader { .. }),
            "expected MigrationError::Loader, got {err:?}"
        );
        // The error message must guide the developer to regenerate.
        let msg = err.to_string();
        assert!(
            msg.contains("sidecar drift") || msg.contains("regenerate"),
            "error message should mention sidecar drift or regenerate; got: {msg}"
        );
    }

    #[test]
    fn load_dir_checked_skips_drift_check_when_no_py_file() {
        // When there is no .py file beside the sidecar (sidecar-only migration),
        // the drift check is skipped — the sidecar is loaded as-is.
        let tmp = tempfile::tempdir().unwrap();

        // Write a sidecar with an arbitrary checksum; no .py companion.
        let spec = make_spec("0001_initial");
        write_sidecar(tmp.path(), "0001_initial", &spec);
        // Deliberately do NOT write a .py file.

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(
            graph.migrations.len(),
            1,
            "sidecar without .py companion must still be loaded"
        );
    }

    #[test]
    fn load_dir_checked_skips_drift_check_when_no_checksum_in_sidecar() {
        // A sidecar with no `checksum` field cannot be drift-checked; it is
        // accepted unconditionally (same policy as load_dir).
        let tmp = tempfile::tempdir().unwrap();
        let py_text = "class Migration: pass\n";

        let mut spec = make_spec("0001_initial");
        spec.checksum = None; // no checksum embedded
        write_sidecar(tmp.path(), "0001_initial", &spec);
        std::fs::write(tmp.path().join("0001_initial.py"), py_text.as_bytes()).unwrap();

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(graph.migrations.len(), 1);
    }

    #[test]
    fn executable_loader_ignores_non_executable_adoption_archives() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("0001_backfill.adoption.json"),
            br#"{"format":"typebridge.migration-adoption-metadata/v1"}"#,
        )
        .unwrap();
        assert!(load_dir(tmp.path()).unwrap().migrations.is_empty());
    }

    #[test]
    fn released_sidecar_loaders_accept_valid_artifact_larger_than_16_mib() {
        let tmp = tempfile::tempdir().unwrap();
        let mut spec = make_spec("0001_large");
        spec.checksum = None;
        let mut bytes = serde_json::to_vec(&spec).unwrap();
        bytes.resize(16 * 1024 * 1024 + 1, b' ');
        std::fs::write(tmp.path().join("0001_large.json"), bytes).unwrap();
        write_py(tmp.path(), "0001_large");

        assert_eq!(
            load_sidecar(&tmp.path().join("0001_large.py")).unwrap(),
            Some(spec.clone())
        );
        assert_eq!(load_dir(tmp.path()).unwrap().migrations, vec![spec.clone()]);
        assert_eq!(load_dir_checked(tmp.path()).unwrap().migrations, vec![spec]);
    }

    #[test]
    fn released_sidecar_loaders_ignore_more_than_65536_unrelated_entries() {
        let tmp = tempfile::tempdir().unwrap();
        // Cycle across enough seed inodes to stay below conservative
        // per-file hard-link ceilings on every release platform.
        let seeds = (0..128)
            .map(|index| tmp.path().join(format!("unrelated-seed-{index:03}.txt")))
            .collect::<Vec<_>>();
        for seed in &seeds {
            std::fs::write(seed, b"not a migration").unwrap();
        }
        for index in 0..65_537 {
            std::fs::hard_link(
                &seeds[index % seeds.len()],
                tmp.path().join(format!("unrelated-{index:05}.txt")),
            )
            .unwrap();
        }
        let mut spec = make_spec("0001_initial");
        spec.checksum = None;
        write_sidecar(tmp.path(), "0001_initial", &spec);
        write_py(tmp.path(), "0001_initial");

        assert_eq!(load_dir(tmp.path()).unwrap().migrations, vec![spec.clone()]);
        assert_eq!(load_dir_checked(tmp.path()).unwrap().migrations, vec![spec]);
    }

    #[cfg(unix)]
    #[test]
    fn released_sidecar_loaders_follow_regular_file_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let py_text = "class Migration: pass\n";
        let spec = make_spec_with_real_checksum("0001_linked", py_text);
        std::fs::write(tmp.path().join("source.py"), py_text).unwrap();
        std::fs::write(
            tmp.path().join("source.json"),
            serde_json::to_vec(&spec).unwrap(),
        )
        .unwrap();
        symlink("source.py", tmp.path().join("0001_linked.py")).unwrap();
        symlink("source.json", tmp.path().join("0001_linked.json")).unwrap();

        assert_eq!(
            load_sidecar(&tmp.path().join("0001_linked.py")).unwrap(),
            Some(spec.clone())
        );
        assert_eq!(load_dir(tmp.path()).unwrap().migrations, vec![spec.clone()]);
        assert_eq!(load_dir_checked(tmp.path()).unwrap().migrations, vec![spec]);
    }
}
