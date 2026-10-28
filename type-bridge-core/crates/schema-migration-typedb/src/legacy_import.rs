//! Verify the permanent applied-ledger binding of already adopted histories.

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_migration::AppliedMigrationRecord;
use type_bridge_schema_migration::{
    LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationId, LegacyMigrationReference,
};

/// Digest semantic rows from the released applied ledger.
///
/// Row order and `applied_at` are deliberately ignored; exact application
/// label, migration name, and checksum spellings remain bound.
pub fn digest_legacy_applied_records(
    applied: &[AppliedMigrationRecord],
) -> Result<LegacyAppliedSetDigest, Diagnostic> {
    LegacyAppliedSetDigest::compute(
        applied
            .iter()
            .map(|record| {
                reference_from_parts(
                    &record.app_label,
                    &record.name,
                    Some(record.checksum.as_str()),
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
}

fn legacy_migration_id(app_label: &str, name: &str) -> Result<LegacyMigrationId, Diagnostic> {
    LegacyMigrationId::new(app_label.to_owned(), name.to_owned())
}

fn reference_from_parts(
    app_label: &str,
    name: &str,
    checksum: Option<&str>,
) -> Result<LegacyMigrationReference, Diagnostic> {
    let checksum = checksum.ok_or_else(|| {
        failure(
            "migration_legacy_import_missing_checksum",
            "legacy migration carries no recorded checksum",
        )
        .with_detail("migration", legacy_key(app_label, name))
    })?;
    Ok(LegacyMigrationReference::new(
        legacy_migration_id(app_label, name)?,
        LegacyMigrationChecksum::new(checksum)?,
    ))
}

fn legacy_key(app_label: &str, name: &str) -> String {
    format!("{app_label}:{name}")
}

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static legacy import diagnostic code"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(name: &str, checksum: &str) -> AppliedMigrationRecord {
        AppliedMigrationRecord {
            app_label: "example".to_owned(),
            name: name.to_owned(),
            checksum: checksum.to_owned(),
            applied_at: None,
        }
    }

    #[test]
    fn complete_applied_digest_ignores_order_and_timestamp_but_detects_every_row_drift() {
        let expected = LegacyAppliedSetDigest::compute([
            LegacyMigrationReference::new(
                LegacyMigrationId::new("example", "0001_initial").unwrap(),
                LegacyMigrationChecksum::new("0123456789abcdef").unwrap(),
            ),
            LegacyMigrationReference::new(
                LegacyMigrationId::new("example", "0002_addresses").unwrap(),
                LegacyMigrationChecksum::new("fedcba9876543210").unwrap(),
            ),
        ])
        .expect("expected applied set digest");
        let mut first = applied("0001_initial", "0123456789abcdef");
        first.applied_at = Some("2020-01-01T00:00:00.000000".to_owned());
        let mut second = applied("0002_addresses", "fedcba9876543210");
        second.applied_at = Some("2099-12-31T23:59:59.999999".to_owned());
        assert_eq!(
            digest_legacy_applied_records(&[second.clone(), first.clone()])
                .expect("reordered ledger digest"),
            expected,
        );

        assert_ne!(
            digest_legacy_applied_records(&[first.clone()]).expect("missing row digest"),
            expected,
        );
        let mut extra = applied("0003_extra", "aaaaaaaaaaaaaaaa");
        extra.app_label = "应用 Ω".to_owned();
        extra.name = "迁移 🚀".to_owned();
        assert_ne!(
            digest_legacy_applied_records(&[first.clone(), second.clone(), extra])
                .expect("extra UTF-8 row digest"),
            expected,
        );
        second.checksum = "aaaaaaaaaaaaaaaa".to_owned();
        assert_ne!(
            digest_legacy_applied_records(&[first, second]).expect("checksum drift digest"),
            expected,
        );
    }
}
