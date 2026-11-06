//! One-time, fenced copy of the frozen journal and verified legacy partition.
use super::*;
use crate::control_schema::{
    LEGACY_CUTOVER_ENTITY, LEGACY_CUTOVER_FINGERPRINT, LEGACY_CUTOVER_KEY, LEGACY_CUTOVER_SCOPE,
    LEGACY_CUTOVER_SINGLETON_KEY,
};
use crate::legacy_ledger::{
    LegacyCutoverSentinelExpectation, load_verified_legacy_partition_in_transaction,
};
use serde::{Deserialize, Serialize};
use type_bridge_contract::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use type_bridge_contract::limits::CodecLimits;
use type_bridge_contract::reserved::{
    IMPORT_COMPLETE_RECORD_KIND, IMPORT_LEGACY_JOURNAL_CONTROL_SCHEMA_TYPEQL,
    LEGACY_IMPORT_RECORD_KIND,
};

const FORMAT: &str = "typebridge.migration-journal-import/v1";
const LIMITS: CodecLimits = CodecLimits {
    max_bytes: 1024 * 1024,
    max_depth: 16,
    max_collection_len: 65_536,
    max_string_bytes: 1024 * 1024,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyEvidence {
    pub(super) applied_set: String,
    pub(super) sentinel_fingerprint: String,
    pub(super) bridge_app: String,
    pub(super) bridge_name: String,
    pub(super) manifest_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ImportComplete {
    format: String,
    scope: String,
    source_database: Option<String>,
    source_next_sequence: u64,
    source_fence: u64,
    source_records_digest: String,
    legacy: Option<LegacyEvidence>,
}

struct ImportSource {
    database: Option<String>,
    control: ControlSnapshot,
    rows: Vec<ImportRow>,
}

struct ImportRow {
    key: String,
    sequence: u64,
    kind: String,
    payload: String,
    digest: String,
}

fn pending() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "migration_journal_import_pending",
        "run migration apply, rollback or upgrade-profile to import the legacy journal",
    )
}
fn invalid(message: &'static str) -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "migration_journal_import_invalid",
        message,
    )
}

async fn legacy_read(
    transaction: &mut Transaction,
    kind: &str,
    fixed: &[(&str, &str)],
    bound: &[(&str, &str)],
) -> Result<Vec<Value>, Diagnostic> {
    let query = data_plan::read_import_legacy(kind, fixed, bound)?;
    query_documents(transaction, &query).await
}

impl TypeDbMigrationStore<'_> {
    /// Discover the old journal read-only, using one snapshot for owner, control and rows.
    async fn import_source(
        &self,
        scope: &ExecutionScope,
        include_rows: bool,
    ) -> Result<ImportSource, Diagnostic> {
        let old = self.managed_database.derived_legacy_journal_database();
        if !old.database_exists().await.map_err(map_orm_error)? {
            return Ok(ImportSource {
                database: None,
                control: ControlSnapshot {
                    fence: 0,
                    next_sequence: 0,
                },
                rows: vec![],
            });
        }
        let export = old.schema_text().await.map_err(map_orm_error)?;
        let actual = type_bridge_schema_compat::released_typeql_to_declared_lossless_projection(
            DocumentId::new("import-journal-export.typeql")?,
            &export,
        )
        .map_err(|_| invalid("old journal schema cannot be decoded losslessly"))?;
        let expected = typeql_to_declared(
            DocumentId::new("import-journal-schema.typeql")?,
            IMPORT_LEGACY_JOURNAL_CONTROL_SCHEMA_TYPEQL,
        )
        .map_err(|_| invalid("frozen journal schema cannot be decoded"))?;
        if actual.declared_identity_fingerprint() != expected.declared_identity_fingerprint() {
            return Err(invalid(
                "old journal schema is not the exact frozen journal contract",
            ));
        }
        let mut tx = open_read(&old).await?;
        let result = async {
            let owners = legacy_read(
                &mut tx,
                JOURNAL_OWNER_ENTITY,
                &[],
                &[
                    (JOURNAL_OWNER_KEY, "key"),
                    (JOURNAL_OWNER_MANAGED_DATABASE, "database"),
                    (JOURNAL_OWNER_MANAGED_SCOPE, "scope"),
                ],
            )
            .await?;
            if owners.len() != 1
                || required_scalar(&owners[0], "key")? != JOURNAL_OWNER_SINGLETON_KEY
                || required_scalar(&owners[0], "database")? != self.managed_database.database_name()
                || required_scalar(&owners[0], "scope")? != scope.managed_scope_id().as_str()
            {
                return Err(invalid(
                    "old journal owner differs from the managed database and scope",
                ));
            }
            let controls = legacy_read(
                &mut tx,
                CONTROL_ENTITY,
                &[],
                &[
                    (CONTROL_SCOPE, "scope"),
                    (LEASE_FENCE, "fence"),
                    (NEXT_SEQUENCE, "next"),
                ],
            )
            .await?;
            if controls.len() > 1
                || controls.iter().any(|row| {
                    required_scalar(row, "scope")
                        .map_or(true, |value| value != scope.managed_scope_id().as_str())
                })
            {
                return Err(invalid(
                    "old journal contains foreign or multiple control scopes",
                ));
            }
            let control = parse_control_documents(controls)?.unwrap_or(ControlSnapshot {
                fence: 0,
                next_sequence: 0,
            });
            let mut rows = Vec::new();
            if include_rows {
                for row in legacy_read(
                    &mut tx,
                    JOURNAL_ENTITY,
                    &[],
                    &[
                        (CONTROL_SCOPE, "scope"),
                        (RECORD_KEY, "key"),
                        (RECORD_SEQUENCE, "sequence"),
                        (RECORD_KIND, "kind"),
                        (RECORD_PAYLOAD, "payload"),
                        (RECORD_PAYLOAD_DIGEST, "digest"),
                    ],
                )
                .await?
                {
                    let checked = parse_stored_row(row.clone())?;
                    let sequence = checked.sequence.get();
                    let kind = required_scalar(&row, "kind")?;
                    if matches!(
                        kind.as_str(),
                        IMPORT_COMPLETE_RECORD_KIND | LEGACY_IMPORT_RECORD_KIND
                    ) {
                        return Err(invalid("an old journal cannot contain new import records"));
                    }
                    if required_scalar(&row, "scope")? != scope.managed_scope_id().as_str()
                        || checked.key
                            != format!("{}#{sequence}", scope.managed_scope_id().as_str())
                        || sequence > control.next_sequence
                        || persisted_fence(&checked.payload, &kind)?.get() > control.fence
                    {
                        return Err(invalid(
                            "old journal record lies outside its scope, sequence or fence",
                        ));
                    }
                    rows.push(ImportRow {
                        key: checked.key,
                        sequence,
                        kind,
                        payload: required_scalar(&row, "payload")?,
                        digest: required_scalar(&row, "digest")?,
                    });
                }
                rows.sort_by_key(|row| row.sequence);
                if rows
                    .windows(2)
                    .any(|pair| pair[0].sequence == pair[1].sequence)
                {
                    return Err(invalid(
                        "old journal contains duplicate sequence identities",
                    ));
                }
            }
            Ok(ImportSource {
                database: Some(old.database_name().to_owned()),
                control,
                rows,
            })
        }
        .await;
        finish_read(&mut tx, result).await
    }

    async fn import_complete(
        &self,
        scope: &ExecutionScope,
    ) -> Result<Option<ImportComplete>, Diagnostic> {
        if !self
            .journal_database
            .database_exists()
            .await
            .map_err(map_orm_error)?
        {
            return Ok(None);
        }
        let export = self
            .journal_database
            .schema_text()
            .await
            .map_err(map_orm_error)?;
        if journal_schema_state(&export)? == JournalSchemaState::Empty {
            return Ok(None);
        }
        let mut tx = open_read(&self.journal_database).await?;
        let result = async {
            self.require_journal_owner(&mut tx).await?;
            let rows = load_rows(&mut tx, scope, Some(IMPORT_COMPLETE_RECORD_KIND)).await?;
            if rows.len() > 1 {
                return Err(invalid("journal contains multiple import-complete records"));
            }
            let Some(row) = rows.first() else {
                return Ok(None);
            };
            let value: ImportComplete = from_canonical_json_with_limits(&row.payload, LIMITS)?;
            if value.format != FORMAT || value.scope != scope.managed_scope_id().as_str() {
                return Err(invalid(
                    "import-complete record does not bind this format and scope",
                ));
            }
            self.validate_import_evidence(value.legacy.as_ref())?;
            let proof = load_rows(&mut tx, scope, Some(LEGACY_IMPORT_RECORD_KIND)).await?;
            match (value.legacy.as_ref(), proof.as_slice()) {
                (None, []) => {}
                (Some(expected), [stored]) => {
                    let observed: LegacyEvidence =
                        from_canonical_json_with_limits(&stored.payload, LIMITS)?;
                    if &observed != expected || stored.sequence >= row.sequence {
                        return Err(invalid(
                            "legacy-import proof differs from import-complete evidence",
                        ));
                    }
                }
                _ => {
                    return Err(invalid(
                        "legacy-import proof cardinality differs from import-complete evidence",
                    ));
                }
            }
            if row.sequence.get() <= value.source_next_sequence {
                return Err(invalid(
                    "import-complete does not follow the copied sequence boundary",
                ));
            }
            Ok(Some(value))
        }
        .await;
        finish_read(&mut tx, result).await
    }

    fn validate_import_evidence(
        &self,
        evidence: Option<&LegacyEvidence>,
    ) -> Result<(), Diagnostic> {
        let Some(evidence) = evidence else {
            if self.import_binding.is_some() {
                return Err(invalid(
                    "import-complete is missing the required legacy bridge evidence",
                ));
            }
            return Ok(());
        };
        let bridge = self
            .catalog
            .get(&MigrationId::new(
                &evidence.bridge_app,
                &evidence.bridge_name,
            )?)
            .ok_or_else(|| invalid("imported legacy bridge is absent from the verified catalog"))?;
        if !bridge.is_legacy_bridge()
            || verified_manifest_digest(bridge)?.to_hex() != evidence.manifest_digest
            || bridge
                .legacy_applied_set()
                .is_none_or(|digest| digest.as_str() != evidence.applied_set)
        {
            return Err(invalid(
                "imported legacy evidence differs from the verified bridge manifest",
            ));
        }
        if let Some(binding) = &self.import_binding
            && binding.anchor_fingerprint != evidence.sentinel_fingerprint
        {
            return Err(invalid(
                "imported sentinel differs from the adopted authority fingerprint",
            ));
        }
        Ok(())
    }

    async fn check_old_writer(
        &self,
        scope: &ExecutionScope,
        complete: &ImportComplete,
    ) -> Result<(), Diagnostic> {
        // Deleting the old database after successful import is explicitly allowed.
        let old = self.managed_database.derived_legacy_journal_database();
        if !old.database_exists().await.map_err(map_orm_error)? {
            return Ok(());
        }
        if complete.source_database.as_deref() != Some(old.database_name()) {
            return Err(invalid("an unexpected old journal appeared after import"));
        }
        let mut tx = open_read(&old).await?;
        let read = legacy_read(
            &mut tx,
            CONTROL_ENTITY,
            &[(CONTROL_SCOPE, scope.managed_scope_id().as_str())],
            &[(LEASE_FENCE, "fence"), (NEXT_SEQUENCE, "next")],
        )
        .await;
        let rows = finish_read(&mut tx, read).await?;
        let current = parse_control_documents(rows)?.unwrap_or(ControlSnapshot {
            fence: 0,
            next_sequence: 0,
        });
        if current.next_sequence > complete.source_next_sequence {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_old_writer_after_import",
                "the old journal advanced after import; stop old migration runners and reconcile history",
            ));
        }
        Ok(())
    }

    pub(super) async fn require_import_complete_read_only(
        &self,
        scope: &ExecutionScope,
    ) -> Result<(), Diagnostic> {
        let complete = self.import_complete(scope).await?.ok_or_else(pending)?;
        self.check_old_writer(scope, &complete).await
    }

    async fn legacy_evidence(&self) -> Result<Option<LegacyEvidence>, Diagnostic> {
        let mut tx = self
            .managed_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        let result = async {
            let expectation = match &self.import_binding {
                Some(binding) => {
                    LegacyCutoverSentinelExpectation::RequiredExact(&binding.anchor_fingerprint)
                }
                None => LegacyCutoverSentinelExpectation::Absent,
            };
            let partition = load_verified_legacy_partition_in_transaction(&mut tx, expectation)
                .await
                .map_err(|error| {
                    let code = if error.is_contract_violation() {
                        "migration_journal_import_invalid"
                    } else {
                        "migration_legacy_applied_set_unreadable"
                    };
                    failure(
                        DiagnosticCategory::Integrity,
                        code,
                        "legacy applied partition or cutover sentinel cannot be verified",
                    )
                    .with_detail("legacy", error.to_string())
                })?;
            let Some(binding) = &self.import_binding else {
                if !partition.applied().is_empty() {
                    return Err(invalid(
                        "legacy applied rows require a verified adopted bridge",
                    ));
                }
                return Ok(None);
            };
            let applied_set =
                crate::legacy_import::digest_legacy_applied_records(partition.applied())?;
            if applied_set != binding.expected_applied_set {
                return Err(invalid(
                    "legacy applied set differs from the verified bridge",
                ));
            }
            let query = data_plan::read_import_legacy(
                LEGACY_CUTOVER_ENTITY,
                &[(LEGACY_CUTOVER_KEY, LEGACY_CUTOVER_SINGLETON_KEY)],
                &[
                    (LEGACY_CUTOVER_SCOPE, "scope"),
                    (LEGACY_CUTOVER_FINGERPRINT, "fingerprint"),
                ],
            )?;
            let anchor = crate::runner::legacy_anchor_plan_values(&mut tx, query).await?;
            if anchor.len() != 1
                || required_scalar(&anchor[0], "scope")? != self.managed_scope_id.as_str()
                || required_scalar(&anchor[0], "fingerprint")? != binding.anchor_fingerprint
            {
                return Err(invalid(
                    "legacy anchor does not match its verified scope and sentinel",
                ));
            }
            let bridge = self
                .catalog
                .values()
                .find(|manifest| manifest.is_legacy_bridge())
                .ok_or_else(|| invalid("legacy import has no verified bridge manifest"))?;
            Ok(Some(LegacyEvidence {
                applied_set: applied_set.as_str().to_owned(),
                sentinel_fingerprint: partition
                    .sentinel_fingerprint()
                    .ok_or_else(|| invalid("verified legacy sentinel is missing"))?
                    .to_owned(),
                bridge_app: bridge.id().app_label().as_str().to_owned(),
                bridge_name: bridge.id().name().as_str().to_owned(),
                manifest_digest: verified_manifest_digest(bridge)?.to_hex(),
            }))
        }
        .await;
        let close = tx.close().await;
        let value = result?;
        close.map_err(map_orm_error)?;
        Ok(value)
    }
}

impl TypeDbMigrationStore<'_> {
    pub(super) async fn acquire_imported(
        &self,
        scope: &ExecutionScope,
        holder: &LeaseHolderId,
    ) -> Result<(MigrationLease, Option<LegacyEvidence>), Diagnostic> {
        let completed = self.import_complete(scope).await?;
        let source = if completed.is_none() {
            Some(self.import_source(scope, true).await?)
        } else {
            None
        };
        let floor = source.as_ref().map_or_else(
            || completed.as_ref().map_or(0, |value| value.source_fence),
            |value| value.control.fence,
        );
        let lease = self.acquire_authoritative(scope, holder, floor).await?;
        // A competing importer may have completed before this lease took over.
        if let Some(complete) = self.import_complete(scope).await? {
            self.check_old_writer(scope, &complete).await?;
            return Ok((lease, complete.legacy));
        }
        let source = source
            .ok_or_else(|| invalid("completed import disappeared during lease acquisition"))?;
        let legacy = self.legacy_evidence().await?;
        self.reset_import(&lease).await?;
        for row in &source.rows {
            self.copy_import_row(&lease, row).await?;
        }
        self.set_import_sequence(&lease, source.control.next_sequence)
            .await?;
        if let Some(evidence) = &legacy {
            let payload = to_canonical_json_with_limits(evidence, LIMITS)?;
            self.append_import_record(&lease, LEGACY_IMPORT_RECORD_KIND, &payload)
                .await?;
            // A legacy-only partition has no canonical applied record to copy.
            // Its verified bridge becomes the applied baseline exactly once.
            if source.database.is_none() {
                let manifest = self
                    .catalog
                    .get(&MigrationId::new(
                        &evidence.bridge_app,
                        &evidence.bridge_name,
                    )?)
                    .ok_or_else(|| invalid("legacy bridge disappeared from catalog"))?;
                let record = AppliedRecord::from_verified_manifest_contract(&lease, manifest)?;
                self.append_import_record(&lease, APPLIED_RECORD_KIND, &encode_applied(&record)?)
                    .await?;
            }
        }
        let complete = ImportComplete {
            format: FORMAT.to_owned(),
            scope: scope.managed_scope_id().as_str().to_owned(),
            source_database: source.database,
            source_next_sequence: source.control.next_sequence,
            source_fence: source.control.fence,
            source_records_digest: import_rows_digest(&source.rows),
            legacy,
        };
        self.check_old_writer(scope, &complete).await?;
        let payload = to_canonical_json_with_limits(&complete, LIMITS)?;
        self.append_import_record(&lease, IMPORT_COMPLETE_RECORD_KIND, &payload)
            .await?;
        Ok((lease, complete.legacy))
    }

    async fn reset_import(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        let mut tx = Transaction::open(
            &self.journal_database,
            TransactionKind::Write,
            TransactionOptions::default(),
        )
        .await
        .map_err(map_orm_error)?;
        let current = load_active_control(&mut tx, lease).await?;
        let delete = data_plan::delete_kinds(
            JOURNAL_ENTITY,
            CONTROL_SCOPE,
            lease.scope().managed_scope_id().as_str(),
            RECORD_KIND,
            &[
                PLAN_RECORD_KIND,
                EVENT_RECORD_KIND,
                BACKFILL_EVENT_RECORD_KIND,
                APPLIED_RECORD_KIND,
                ROLLBACK_PLAN_RECORD_KIND,
                ROLLBACK_EVENT_RECORD_KIND,
                ROLLED_BACK_RECORD_KIND,
                LEGACY_IMPORT_RECORD_KIND,
                IMPORT_COMPLETE_RECORD_KIND,
            ],
        )?;
        execute_data_plan(&mut tx, &delete).await?;
        replace_sequence(&mut tx, lease, current.next_sequence, 0).await?;
        tx.commit().await.map_err(map_orm_error)
    }

    async fn set_import_sequence(
        &self,
        lease: &MigrationLease,
        sequence: u64,
    ) -> Result<(), Diagnostic> {
        let mut tx = Transaction::open(
            &self.journal_database,
            TransactionKind::Write,
            TransactionOptions::default(),
        )
        .await
        .map_err(map_orm_error)?;
        let current = load_active_control(&mut tx, lease).await?;
        if current.next_sequence > sequence {
            return Err(invalid("copied journal sequence exceeds source boundary"));
        }
        replace_sequence(&mut tx, lease, current.next_sequence, sequence).await?;
        tx.commit().await.map_err(map_orm_error)
    }

    async fn copy_import_row(
        &self,
        lease: &MigrationLease,
        row: &ImportRow,
    ) -> Result<(), Diagnostic> {
        let mut tx = Transaction::open(
            &self.journal_database,
            TransactionKind::Write,
            TransactionOptions::default(),
        )
        .await
        .map_err(map_orm_error)?;
        let current = load_active_control(&mut tx, lease).await?;
        let expected = control_fields(lease, current.next_sequence);
        let replacement = control_fields(lease, row.sequence);
        let expected: Vec<_> = expected
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let replacement: Vec<_> = replacement
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let plan = data_plan::replace_and_insert(
            CONTROL_ENTITY,
            &expected,
            &replacement,
            JOURNAL_ENTITY,
            &[
                (RECORD_KEY, &row.key),
                (CONTROL_SCOPE, lease.scope().managed_scope_id().as_str()),
                (RECORD_SEQUENCE, &row.sequence.to_string()),
                (RECORD_KIND, &row.kind),
                (RECORD_PAYLOAD, &row.payload),
                (RECORD_PAYLOAD_DIGEST, &row.digest),
            ],
        )?;
        require_one(&mut tx, &plan).await?;
        tx.commit().await.map_err(map_orm_error)
    }

    async fn append_import_record(
        &self,
        lease: &MigrationLease,
        kind: &str,
        payload: &[u8],
    ) -> Result<(), Diagnostic> {
        let mut tx = Transaction::open(
            &self.journal_database,
            TransactionKind::Write,
            TransactionOptions::default(),
        )
        .await
        .map_err(map_orm_error)?;
        let current = load_active_control(&mut tx, lease).await?;
        append_record(&mut tx, lease, current, kind, payload).await?;
        tx.commit().await.map_err(map_orm_error)
    }
}

fn control_fields(lease: &MigrationLease, sequence: u64) -> Vec<(&'static str, String)> {
    vec![
        (
            CONTROL_SCOPE,
            lease.scope().managed_scope_id().as_str().to_owned(),
        ),
        (LEASE_HOLDER, lease.holder().as_str().to_owned()),
        (LEASE_FENCE, lease.fence().get().to_string()),
        (LEASE_STATE, LEASE_HELD.to_owned()),
        (NEXT_SEQUENCE, sequence.to_string()),
    ]
}
async fn require_one(tx: &mut Transaction, plan: &QueryPlan) -> Result<(), Diagnostic> {
    let answers = tx.execute_plan(plan).await.map_err(map_orm_error)?;
    if !matches!(answers.as_slice(), [PlanAnswer::Matched(1)]) {
        return Err(stale_fence());
    }
    Ok(())
}
async fn replace_sequence(
    tx: &mut Transaction,
    lease: &MigrationLease,
    before: u64,
    after: u64,
) -> Result<(), Diagnostic> {
    let expected = control_fields(lease, before);
    let replacement = control_fields(lease, after);
    let expected: Vec<_> = expected
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let replacement: Vec<_> = replacement
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let plan = data_plan::replace(CONTROL_ENTITY, &expected, &replacement)?;
    require_one(tx, &plan).await
}
fn import_rows_digest(rows: &[ImportRow]) -> String {
    let mut hash = Sha256::new();
    hash.update(FORMAT.as_bytes());
    for row in rows {
        hash.update(row.sequence.to_be_bytes());
        for value in [&row.key, &row.kind, &row.payload, &row.digest] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
    }
    format!("{:x}", hash.finalize())
}

impl TypeDbMigrationStore<'_> {
    pub(super) async fn publish_imported_mirror(
        &self,
        lease: &MigrationLease,
        plan: &QueryPlan,
        evidence: &LegacyEvidence,
    ) -> Result<(), Diagnostic> {
        let mut tx = Transaction::open(
            &self.managed_database,
            TransactionKind::Write,
            TransactionOptions::default(),
        )
        .await
        .map_err(map_orm_error)?;
        require_one(&mut tx, plan).await?;
        let query = data_plan::read_managed_anchor(&[
            (LEGACY_CUTOVER_KEY, "key"),
            (LEGACY_CUTOVER_SCOPE, "scope"),
            (LEGACY_CUTOVER_FINGERPRINT, "fingerprint"),
        ])?;
        let anchors = query_documents(&mut tx, &query).await?;
        match anchors.as_slice() {
            [] => {
                let insert = data_plan::insert_managed_anchor(&[
                    (LEGACY_CUTOVER_KEY, LEGACY_CUTOVER_SINGLETON_KEY),
                    (
                        LEGACY_CUTOVER_SCOPE,
                        lease.scope().managed_scope_id().as_str(),
                    ),
                    (LEGACY_CUTOVER_FINGERPRINT, &evidence.sentinel_fingerprint),
                ])?;
                execute_data_plan(&mut tx, &insert).await?;
            }
            [anchor]
                if required_scalar(anchor, "key")? == LEGACY_CUTOVER_SINGLETON_KEY
                    && required_scalar(anchor, "scope")?
                        == lease.scope().managed_scope_id().as_str()
                    && required_scalar(anchor, "fingerprint")? == evidence.sentinel_fingerprint => {
            }
            _ => {
                return Err(invalid(
                    "new managed cutover anchor differs from imported evidence",
                ));
            }
        }
        tx.commit().await.map_err(map_orm_error)
    }
}
