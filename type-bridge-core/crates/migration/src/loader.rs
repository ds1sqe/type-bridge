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
/// [`MigrationGraph`], then validate that each sidecar's embedded checksum
/// agrees with the current `.py` text.
///
/// This is the **checked** variant of [`load_dir`], intended for use by the
/// Rust CLI whenever it is the execution source.  The check guards against
/// sidecar drift: if a developer hand-edits the `.py` after the sidecar was
/// generated, the sidecar's `checksum` field will disagree with the fresh
/// `.py` text, and this function returns an error rather than silently
/// executing a stale sidecar.
///
/// # Invariant
///
/// The `.py` text is the sole checksum source (sub-plan 04/07 invariant).
/// The sidecar carries a copy of that checksum so the drift guard can compare
/// without importing Python; if the `.py` file is absent for a given sidecar
/// the check is skipped (legacy sidecar-only migration; no `.py` to compare).
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when:
/// - The directory or a sidecar cannot be read (same as [`load_dir`]).
/// - A sidecar's embedded `checksum` disagrees with the recomputed `.py` text
///   checksum — "sidecar drift: regenerate the migration" (D6 guard).
pub fn load_dir_checked(dir: &Path) -> Result<MigrationGraph> {
    let read_dir = std::fs::read_dir(dir).map_err(|err| MigrationError::Loader {
        message: format!("failed to read migrations dir {}: {err}", dir.display()),
    })?;

    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|err| MigrationError::Loader {
            message: format!("failed to iterate migrations dir {}: {err}", dir.display()),
        })?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };

        if !is_migration_stem(&stem) {
            continue;
        }

        entries.push((stem, path));
    }

    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

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

        // D6 drift guard: recompute the .py text checksum and compare to the
        // sidecar's embedded value.  The .py text is the sole checksum source
        // (04/07 invariant); the sidecar is a generated cache.  If the .py
        // was hand-edited after the sidecar was written, the checksums will
        // diverge and we reject the stale sidecar rather than executing it.
        if let Some(sidecar_checksum) = &spec.checksum {
            let py_path = json_path.with_extension("py");
            if py_path.exists() {
                let py_text =
                    std::fs::read_to_string(&py_path).map_err(|err| MigrationError::Loader {
                        message: format!(
                            "failed to read .py for drift check {}: {err}",
                            py_path.display()
                        ),
                    })?;
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

/// Return `true` if the stem matches the migration naming convention:
/// four ASCII digits followed by an underscore and at least one more character.
fn is_migration_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    if bytes.len() < 6 {
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
}
