//! Shape grouping for generated bulk writes; shared by every SDK.
use super::*;
use crate::query_remote::RemoteFieldValue;
use std::cmp::Ordering;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Debug)]
struct ScalarKey<'a>(&'a CanonicalValue);

impl PartialEq for ScalarKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for ScalarKey<'_> {}
impl PartialOrd for ScalarKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScalarKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .value_type()
            .cmp(&other.0.value_type())
            .then_with(|| {
                self.0
                    .semantic_cmp_same_domain(other.0)
                    .unwrap_or_else(|| self.0.cmp(other.0))
            })
    }
}

fn result_error() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "query_remote_document_value",
        "bulk documents do not match their input rows",
    )
}

/// Same-shape rows in first-appearance order, with their original positions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkRows {
    /// Owned fields, in canonical order, present in every row of this group.
    pub fields: Vec<OwnsFactId>,
    /// Required values in the same order as `fields`.
    pub values: Vec<Vec<CanonicalValue>>,
    /// Original input index of each row, for result-order restoration.
    pub positions: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct BulkChunk {
    pub group: usize,
    pub range: Range<usize>,
}

/// Borrowed correlation evidence for one executed chunk, without copying values.
#[derive(Clone, Copy, Debug)]
pub struct BulkRowSlice<'a> {
    /// Owned fields shared by every row of the chunk.
    pub fields: &'a [OwnsFactId],
    /// Canonical values borrowed from the original input group.
    pub values: &'a [Vec<CanonicalValue>],
    /// Original input positions borrowed from the same group.
    pub positions: &'a [usize],
}

/// One authored call's validated plans and optional bulk correlation evidence.
/// Stage every plan in the same transaction before resolving its result.
#[derive(Clone, Debug)]
pub struct BuiltPlans {
    plans: Vec<QueryPlan>,
    bulk: Option<Arc<Vec<BulkRows>>>,
    chunks: Vec<BulkChunk>,
    documents: bool,
    projection: Option<QueryPlan>,
}

impl AsRef<[QueryPlan]> for BuiltPlans {
    fn as_ref(&self) -> &[QueryPlan] {
        self.plans()
    }
}

impl BuiltPlans {
    /// A plain, already validated plan without bulk correlation evidence.
    #[must_use]
    pub fn single(plan: QueryPlan) -> Self {
        let documents = plan.fetch_document_shape().is_some();
        Self {
            plans: vec![plan],
            bulk: None,
            chunks: vec![],
            documents,
            projection: None,
        }
    }

    pub(super) fn bulk(
        plans: Vec<QueryPlan>,
        rows: Arc<Vec<BulkRows>>,
        chunks: Vec<BulkChunk>,
        projection: QueryPlan,
    ) -> Self {
        Self {
            plans,
            bulk: Some(rows),
            chunks,
            documents: projection.fetch_document_shape().is_some(),
            projection: Some(projection),
        }
    }

    /// Plans in first-seen shape and then chunk order; empty bulk has none.
    #[must_use]
    pub fn plans(&self) -> &[QueryPlan] {
        &self.plans
    }

    /// Executed fetch evidence, or the validated schema-only skeleton of empty bulk.
    #[must_use]
    pub fn projection_plan(&self) -> &QueryPlan {
        self.plans
            .first()
            .or(self.projection.as_ref())
            .expect("built plans retain projection evidence")
    }

    /// Check a generated model result before any transaction can be staged.
    pub fn validate_model_projection(&self) -> Result<(), Diagnostic> {
        if !self.documents {
            return Err(invalid(
                "plan_builder_model_projection",
                "a model result requires fetched documents",
            ));
        }
        for plan in self.plans.iter().chain(self.projection.iter()) {
            plan.validate_model_projection()?;
        }
        Ok(())
    }

    /// Immutable source groups and their original positions, including empty bulk.
    #[must_use]
    pub fn bulk_rows(&self) -> Option<&[BulkRows]> {
        self.bulk.as_deref().map(Vec::as_slice)
    }

    /// Borrow each executed chunk from the immutable grouped source rows.
    pub fn bulk_chunks(&self) -> Option<impl ExactSizeIterator<Item = BulkRowSlice<'_>> + Clone> {
        self.bulk.as_ref().map(|groups| {
            self.chunks.iter().map(move |chunk| {
                let group = &groups[chunk.group];
                BulkRowSlice {
                    fields: &group.fields,
                    values: &group.values[chunk.range.clone()],
                    positions: &group.positions[chunk.range.clone()],
                }
            })
        })
    }

    #[cfg(test)]
    pub(super) fn bulk_source(&self) -> Option<&Arc<Vec<BulkRows>>> {
        self.bulk.as_ref()
    }

    /// Whether this call selects fetched documents, including when empty.
    #[must_use]
    pub const fn returns_documents(&self) -> bool {
        self.documents
    }

    /// Whether the authored call is a write; an empty bulk remains a write.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.bulk.is_some() || self.plans.iter().any(QueryPlan::writes)
    }
}

impl QueryPlan {
    /// Check that the terminal fetch supports a complete generated model result.
    pub fn validate_model_projection(&self) -> Result<(), Diagnostic> {
        let shape = self.fetch_document_shape().ok_or_else(|| {
            invalid(
                "plan_builder_model_projection",
                "a model result requires fetched documents",
            )
        })?;
        if self.writes() && !shape.supports_write_model_projection() {
            return Err(invalid(
                "plan_returning_models_after_write",
                "model results after writes require one complete supported fetch",
            ));
        }
        Ok(())
    }
}

fn standalone_entity(pipeline: &[Stage]) -> Option<&InsertThing> {
    let (write, fetch) = match pipeline {
        [write] => (write, None),
        [write, Stage::Fetch(fetch)] => (write, Some(fetch)),
        _ => return None,
    };
    let (Stage::Insert { things } | Stage::Put { things, .. }) = write else {
        return None;
    };
    let [thing] = things.as_slice() else {
        return None;
    };
    (thing.type_id.kind() == TypeKind::Entity
        && thing.players.is_empty()
        && fetch.is_none_or(|fetch| fetch.thing == thing.binding))
    .then_some(thing)
}

pub(super) fn standalone_scalar_entity(spec: &PlanSpec, schema: &DeclaredSchema) -> bool {
    let Some(thing) = standalone_entity(&spec.pipeline) else {
        return false;
    };
    let Some(profile) = spec.managed_semantics.as_fingerprint().semantic_profile() else {
        return false;
    };
    let Ok(profile) = crate::semantic_profile::SemanticProfile::resolve(profile) else {
        return false;
    };
    if SchemaIndex::new(schema).has_list_ownership(&thing.type_id, &profile) {
        return false;
    }
    let mut fields = BTreeSet::new();
    thing.fields.iter().all(|(field, value)| {
        fields.insert(field)
            && match value {
                QueryOperand::Literal { .. } => true,
                QueryOperand::Input { column } => spec.given_rows.as_ref().is_some_and(|rows| {
                    rows.iter().all(|row| {
                        row.values()
                            .get(usize::from(column.get()))
                            .is_some_and(Option::is_some)
                    })
                }),
                QueryOperand::Binding { .. } => false,
            }
    })
}

/// Whether a write plan puts exactly one thing per input row: its only
/// write is one `Put` of one thing answered by the terminal fetch, and its
/// `Match` (if any) binds only that thing's role players, each by IID or by
/// a key ownership, plus value assignments. Such a plan answers at most one
/// document per input row unless the put pattern matched several instances.
/// With several input rows, the fetch answers an `ordinal` scalar assigned
/// from an input column with a distinct value in every row, which tells the
/// rows' documents apart.
pub(super) fn single_row_put(spec: &PlanSpec, schema: &DeclaredSchema) -> bool {
    let (patterns, put, fetch) = match spec.pipeline.as_slice() {
        [put, Stage::Fetch(fetch)] => (&[][..], put, fetch),
        [Stage::Match { patterns }, put, Stage::Fetch(fetch)] => (patterns.as_slice(), put, fetch),
        _ => return false,
    };
    if let Some(rows) = spec.given_rows.as_ref().filter(|rows| rows.len() > 1)
        && !distinct_row_ordinals(rows, patterns, fetch)
    {
        return false;
    }
    let Stage::Put { things, .. } = put else {
        return false;
    };
    let [thing] = things.as_slice() else {
        return false;
    };
    if fetch.thing != thing.binding {
        return false;
    }
    let players = thing
        .players
        .iter()
        .map(|(_, player)| *player)
        .collect::<BTreeSet<_>>();
    let index = SchemaIndex::new(schema);
    let mut reads = Vec::new();
    let mut by_iid = BTreeSet::new();
    for pattern in patterns {
        match pattern {
            Pattern::Let {
                binding,
                expr: Expr::Operand(_),
            } if !players.contains(binding) => {}
            Pattern::Iid { binding, .. } if players.contains(binding) => {
                by_iid.insert(*binding);
            }
            Pattern::Read(pattern) => reads.push(pattern.clone()),
            _ => return false,
        }
    }
    identified_players(&reads, &players, &index).is_some_and(|mut identified| {
        identified.extend(by_iid);
        identified == players
    })
}

fn distinct_row_ordinals(rows: &[InputRow], patterns: &[Pattern], fetch: &ThingDocument) -> bool {
    let Some(binding) = fetch
        .scalars
        .iter()
        .find(|scalar| scalar.key.as_str() == "ordinal")
        .map(|scalar| scalar.binding)
    else {
        return false;
    };
    let Some(column) = patterns.iter().find_map(|pattern| match pattern {
        Pattern::Let {
            binding: assigned,
            expr: Expr::Operand(QueryOperand::Input { column }),
        } if *assigned == binding => Some(usize::from(column.get())),
        _ => None,
    }) else {
        return false;
    };
    let mut seen = BTreeSet::new();
    rows.iter().all(|row| {
        row.values()
            .get(column)
            .and_then(Option::as_ref)
            .is_some_and(|value| seen.insert(value))
    })
}

/// The players a conjunction identifies by IID or key, or `None` when it
/// contains anything else that could bind or multiply rows.
fn identified_players(
    patterns: &[QueryPattern],
    players: &BTreeSet<BindingId>,
    index: &SchemaIndex<'_>,
) -> Option<BTreeSet<BindingId>> {
    let mut types = BTreeMap::new();
    for pattern in patterns {
        if let QueryPattern::Isa {
            binding,
            include_subtypes: false,
            type_id,
        } = pattern
        {
            if !players.contains(binding) {
                return None;
            }
            types.insert(*binding, type_id);
        }
    }
    // An attribute binding pins its owner only when it equals a given value.
    let given = |operand: &QueryOperand| {
        matches!(
            operand,
            QueryOperand::Literal { .. } | QueryOperand::Input { .. }
        )
    };
    let mut pinned = BTreeSet::new();
    for pattern in patterns {
        if let QueryPattern::Value {
            comparator: ValueComparator::Equal,
            left,
            right,
        } = pattern
        {
            match (left, right) {
                (QueryOperand::Binding { binding }, other)
                | (other, QueryOperand::Binding { binding })
                    if given(other) =>
                {
                    pinned.insert(*binding);
                }
                _ => return None,
            }
        }
    }
    let mut identified = BTreeSet::new();
    for pattern in patterns {
        match pattern {
            QueryPattern::Isa {
                include_subtypes: false,
                ..
            }
            | QueryPattern::Value {
                comparator: ValueComparator::Equal,
                ..
            } => {}
            QueryPattern::Iid { binding, iid } if players.contains(binding) && given(iid) => {
                identified.insert(*binding);
            }
            QueryPattern::Has {
                owner,
                attribute_id,
                attribute,
            } => {
                if !pinned.contains(attribute) {
                    return None;
                }
                let keyed = index.ancestors(types.get(owner)?).into_iter().any(|owner| {
                    OwnsFactId::new(owner, attribute_id.clone()).is_ok_and(|field| {
                        index.annotation(AnnotationSubjectId::Owns(field), AnnotationKindId::Key)
                    })
                });
                if !keyed {
                    return None;
                }
                identified.insert(*owner);
            }
            QueryPattern::Or { branches } => {
                let mut common: Option<BTreeSet<BindingId>> = None;
                for branch in branches {
                    let branch = identified_players(branch, players, index)?;
                    common = Some(match common {
                        None => branch,
                        Some(common) => common.intersection(&branch).copied().collect(),
                    });
                }
                identified.extend(common.unwrap_or_default());
            }
            _ => return None,
        }
    }
    Some(identified)
}

fn put_ambiguous_match() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "put_ambiguous_match",
        "the put pattern matched more than one existing instance",
    )
}

impl QueryPlan {
    /// Refuse a put whose pattern matched several instances for one input
    /// row, before the provider commits. `documents` counts the documents
    /// answered for one row: with several rows, those with its `ordinal`.
    pub fn validate_single_row_put_answer(&self, documents: usize) -> Result<(), Diagnostic> {
        if self.single_row_put && documents > 1 {
            return Err(put_ambiguous_match());
        }
        Ok(())
    }
}

impl QueryPlan {
    /// Expected present-field multiset derived from a standalone scalar entity write.
    /// The evidence is rebuilt from canonical inputs, including on the proxy server.
    #[must_use]
    pub fn standalone_entity_rows(&self) -> Option<BulkRows> {
        if !self.standalone_scalar_entity {
            return None;
        }
        let thing = standalone_entity(&self.pipeline).expect("validated standalone entity shape");
        let row = |row: Option<&InputRow>| {
            thing
                .fields
                .iter()
                .map(|(_, operand)| match operand {
                    QueryOperand::Literal { value } => value.clone(),
                    QueryOperand::Input { column } => row
                        .and_then(|row| row.values().get(usize::from(column.get())))
                        .and_then(Option::as_ref)
                        .expect("validated present input")
                        .clone(),
                    QueryOperand::Binding { .. } => {
                        unreachable!("standalone scalar values have no binding operands")
                    }
                })
                .collect::<Vec<_>>()
        };
        let values = self.given_rows.as_ref().map_or_else(
            || vec![row(None)],
            |rows| rows.iter().map(|input| row(Some(input))).collect(),
        );
        Some(BulkRows {
            fields: thing
                .fields
                .iter()
                .map(|(field, _)| field.clone())
                .collect(),
            positions: (0..values.len()).collect(),
            values,
        })
    }

    /// Refuse a missing or extra standalone entity row before the provider commits.
    pub fn validate_standalone_entity_row_count(&self, count: u64) -> Result<(), Diagnostic> {
        if self.standalone_scalar_entity
            && usize::try_from(count).ok() != Some(self.given_rows.as_ref().map_or(1, Vec::len))
        {
            return Err(result_error());
        }
        Ok(())
    }
}

/// Sealed document ordinals issued only by the bulk correlator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkCorrelation {
    indices: Vec<usize>,
}
impl BulkCorrelation {
    /// Document indexes in original input-row order.
    #[must_use]
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }
}

impl BulkRows {
    /// Correlate schema-validated documents with input rows as a multiset.
    /// The returned indexes are in this group's original row order. Equal
    /// input rows are interchangeable; provider ordering is never assumed.
    ///
    /// # Errors
    /// Refuses missing, extra or non-matching documents, or malformed rows.
    pub fn correlate(
        &self,
        documents: &[BTreeMap<String, RemoteFieldValue>],
    ) -> Result<Vec<usize>, Diagnostic> {
        BulkRowSlice {
            fields: &self.fields,
            values: &self.values,
            positions: &self.positions,
        }
        .correlate(documents)
    }
}
impl BulkRows {
    /// Correlate schema-validated documents with input rows as a multiset.
    /// The returned indexes are in this group's original row order. Equal
    /// input rows are interchangeable; provider ordering is never assumed.
    ///
    /// # Errors
    /// Refuses missing, extra or non-matching documents, or malformed rows.
    pub fn correlate_proof(
        &self,
        documents: &[BTreeMap<String, RemoteFieldValue>],
    ) -> Result<BulkCorrelation, Diagnostic> {
        BulkRowSlice {
            fields: &self.fields,
            values: &self.values,
            positions: &self.positions,
        }
        .correlate_proof(documents)
    }
}
impl BulkRowSlice<'_> {
    /// Correlate already schema-validated documents with this input slice.
    pub fn correlate(
        &self,
        documents: &[BTreeMap<String, RemoteFieldValue>],
    ) -> Result<Vec<usize>, Diagnostic> {
        self.correlate_proof(documents).map(|proof| proof.indices)
    }
    /// Validate correlation and retain its sealed ordinal proof.
    pub fn correlate_proof(
        &self,
        documents: &[BTreeMap<String, RemoteFieldValue>],
    ) -> Result<BulkCorrelation, Diagnostic> {
        let _correlation =
            tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.correlate")
                .entered();
        if documents.len() != self.values.len() || self.positions.len() != self.values.len() {
            return Err(result_error());
        }
        let mut candidates: BTreeMap<Vec<ScalarKey>, Vec<usize>> = BTreeMap::new();
        for (index, document) in documents.iter().enumerate() {
            let Some(RemoteFieldValue::Document { fields }) = document.get("attributes") else {
                return Err(result_error());
            };
            let key = self
                .fields
                .iter()
                .map(
                    |field| match fields.get(field.attribute().label().as_str()) {
                        Some(RemoteFieldValue::Scalar { value }) => Ok(ScalarKey(value)),
                        _ => Err(result_error()),
                    },
                )
                .collect::<Result<Vec<_>, _>>()?;
            candidates.entry(key).or_default().push(index);
        }
        let indices = self
            .values
            .iter()
            .map(|row| {
                if row.len() != self.fields.len() {
                    return Err(result_error());
                }
                let key = row.iter().map(ScalarKey).collect::<Vec<_>>();
                candidates
                    .get_mut(&key)
                    .and_then(Vec::pop)
                    .ok_or_else(result_error)
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        Ok(BulkCorrelation { indices })
    }
}

/// Group bulk rows by present fields, preserving first-seen group order and
/// row order inside each group. Values and schema validation stay with the
/// ordinary plan builder.
///
/// # Errors
/// Refuses duplicate assignments to one owned field in a row.
pub fn group_bulk_rows(
    rows: Vec<Vec<(OwnsFactId, CanonicalValue)>>,
) -> Result<Vec<BulkRows>, Diagnostic> {
    let mut columns = Vec::new();
    let mut indexes = BTreeMap::new();
    let mut indexed = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells = Vec::with_capacity(row.len());
        for (field, value) in row {
            let index = if let Some(index) = indexes.get(&field) {
                *index
            } else {
                let index = columns.len();
                indexes.insert(field.clone(), index);
                columns.push(field);
                index
            };
            cells.push((index, value));
        }
        indexed.push(cells);
    }
    let rows = indexed
        .into_iter()
        .map(|row| {
            let mut cells = vec![None; columns.len()];
            for (index, value) in row {
                if cells[index].replace(value).is_some() {
                    return Err(duplicate_bulk_field());
                }
            }
            Ok(cells)
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    group_bulk_columns(columns, rows)
}

fn duplicate_bulk_field() -> Diagnostic {
    invalid(
        "plan_builder_duplicate_bulk_field",
        "a bulk row assigns one field twice",
    )
}

/// Group one ownership table and optional row cells through the bulk rule set.
/// Field identities are supplied once, not copied or parsed once per row.
pub fn group_bulk_columns(
    columns: Vec<OwnsFactId>,
    rows: Vec<Vec<Option<CanonicalValue>>>,
) -> Result<Vec<BulkRows>, Diagnostic> {
    let mut order = (0..columns.len()).collect::<Vec<_>>();
    order.sort_unstable_by_key(|index| &columns[*index]);
    if order
        .windows(2)
        .any(|pair| columns[pair[0]] == columns[pair[1]])
    {
        return Err(duplicate_bulk_field());
    }
    let mut groups: Vec<BulkRows> = Vec::new();
    let mut shapes = BTreeMap::new();
    for (position, mut row) in rows.into_iter().enumerate() {
        if row.len() != columns.len() {
            return Err(invalid(
                "plan_builder_row_arity",
                "every bulk input row must have one cell per column",
            ));
        }
        let mut fields = Vec::new();
        let mut values = Vec::new();
        for index in &order {
            if let Some(value) = row[*index].take() {
                fields.push(*index);
                values.push(value);
            }
        }
        let group = if let Some(group) = shapes.get(&fields) {
            *group
        } else {
            let group = groups.len();
            groups.push(BulkRows {
                fields: fields.iter().map(|index| columns[*index].clone()).collect(),
                values: vec![],
                positions: vec![],
            });
            shapes.insert(fields, group);
            group
        };
        groups[group].values.push(values);
        groups[group].positions.push(position);
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::{SplitLimits, SplitUnit, split};
    use crate::value::CanonicalString;

    fn field(label: &str) -> OwnsFactId {
        OwnsFactId::new(
            TypeId::new(TypeKind::Entity, "item").unwrap(),
            AttributeId::new(label).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn interleaved_shapes_and_limit_chunks_retain_input_positions() {
        let a = || vec![(field("price"), CanonicalValue::Long(3))];
        let b = || {
            vec![
                (field("stock"), CanonicalValue::Long(5)),
                (field("price"), CanonicalValue::Long(4)),
            ]
        };
        let groups = group_bulk_rows(vec![a(), b(), a(), a()]).unwrap();
        assert_eq!(
            groups
                .iter()
                .map(|group| group.positions.clone())
                .collect::<Vec<_>>(),
            vec![vec![0, 2, 3], vec![1]]
        );
        assert_eq!(groups[1].fields, vec![field("price"), field("stock")]);
        assert_eq!(
            groups[1].values,
            vec![vec![CanonicalValue::Long(4), CanonicalValue::Long(5)]]
        );
        let ranges = split(
            10,
            &vec![SplitUnit { rows: 1, bytes: 5 }; groups[0].values.len()],
            SplitLimits::new(2, 100),
        )
        .unwrap();
        assert_eq!(
            ranges
                .iter()
                .map(|range| groups[0].positions[range.clone()].to_vec())
                .collect::<Vec<_>>(),
            vec![vec![0, 2], vec![3]]
        );
    }

    #[test]
    fn column_table_matches_rows_and_validates_its_shape() {
        let columns = vec![field("stock"), field("price")];
        let cells = vec![
            vec![None, Some(CanonicalValue::Long(3))],
            vec![Some(CanonicalValue::Long(5)), Some(CanonicalValue::Long(4))],
            vec![None, Some(CanonicalValue::Long(3))],
            vec![None, None],
        ];
        let rows = cells
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .zip(row)
                    .filter_map(|(field, value)| value.clone().map(|value| (field.clone(), value)))
                    .collect()
            })
            .collect();
        assert_eq!(
            group_bulk_columns(columns.clone(), cells).unwrap(),
            group_bulk_rows(rows).unwrap()
        );
        assert_eq!(
            group_bulk_columns(columns, vec![vec![]])
                .unwrap_err()
                .code()
                .as_str(),
            "plan_builder_row_arity"
        );
        assert_eq!(
            group_bulk_columns(vec![field("price"), field("price")], vec![])
                .unwrap_err()
                .code()
                .as_str(),
            "plan_builder_duplicate_bulk_field"
        );
    }

    #[test]
    fn ten_thousand_rows_share_three_ownership_identities() {
        let columns = vec![field("item-id"), field("price"), field("stock")];
        let metadata_bytes: usize = columns
            .iter()
            .map(|field| serde_json::to_vec(field).unwrap().len())
            .sum();
        let groups = group_bulk_columns(
            columns.clone(),
            (0..10_000)
                .map(|index| {
                    vec![
                        Some(CanonicalValue::String(
                            CanonicalString::new(format!("I{index}")).unwrap(),
                        )),
                        Some(CanonicalValue::Long(3)),
                        Some(CanonicalValue::Long(10)),
                    ]
                })
                .collect(),
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].fields.len(), 3);
        assert_eq!(groups[0].values.len(), 10_000);
        assert_eq!(groups[0].positions, (0..10_000).collect::<Vec<_>>());
        println!(
            "ownership JSON bytes parsed: {} -> {metadata_bytes}; identities: 30000 -> 3",
            metadata_bytes * 10_000
        );
    }

    #[test]
    fn empty_input_empty_shape_and_duplicate_fields_are_explicit() {
        assert!(group_bulk_rows(vec![]).unwrap().is_empty());
        assert_eq!(group_bulk_rows(vec![vec![]]).unwrap()[0].positions, vec![0]);
        assert_eq!(
            group_bulk_rows(vec![vec![
                (field("price"), CanonicalValue::Long(1)),
                (field("price"), CanonicalValue::Long(2))
            ]])
            .unwrap_err()
            .code()
            .as_str(),
            "plan_builder_duplicate_bulk_field"
        );
    }
}

#[cfg(test)]
mod correlation_tests {
    use super::*;
    use crate::temporal::CanonicalDateTimeTz;
    use crate::value::CanonicalDouble;

    fn field() -> OwnsFactId {
        OwnsFactId::new(
            TypeId::new(TypeKind::Entity, "item").unwrap(),
            AttributeId::new("price").unwrap(),
        )
        .unwrap()
    }
    fn document(value: CanonicalValue) -> BTreeMap<String, RemoteFieldValue> {
        BTreeMap::from([(
            "attributes".to_owned(),
            RemoteFieldValue::Document {
                fields: BTreeMap::from([("price".to_owned(), RemoteFieldValue::Scalar { value })]),
            },
        )])
    }
    #[test]
    fn unordered_documents_duplicate_rows_and_mismatches_are_checked() {
        let group = BulkRows {
            fields: vec![field()],
            positions: vec![0, 2, 3],
            values: vec![
                vec![CanonicalValue::Long(1)],
                vec![CanonicalValue::Long(2)],
                vec![CanonicalValue::Long(1)],
            ],
        };
        let docs = vec![
            document(CanonicalValue::Long(2)),
            document(CanonicalValue::Long(1)),
            document(CanonicalValue::Long(1)),
        ];
        let indexes = group.correlate(&docs).unwrap();
        assert_eq!(indexes, vec![2, 0, 1]);
        for wrong in [
            docs[..2].to_vec(),
            [docs.clone(), vec![document(CanonicalValue::Long(1))]].concat(),
            vec![
                document(CanonicalValue::Long(3)),
                docs[1].clone(),
                docs[2].clone(),
            ],
        ] {
            assert_eq!(
                group.correlate(&wrong).unwrap_err().code().as_str(),
                "query_remote_document_value"
            );
        }
    }
    #[test]
    fn correlation_uses_semantic_double_zero_and_timezone_equality() {
        for (input, output) in [
            (
                CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap()),
                CanonicalValue::Double(CanonicalDouble::new(0.0).unwrap()),
            ),
            (
                CanonicalValue::DateTimeTz(
                    "2026-01-01T09:00:00+09:00"
                        .parse::<CanonicalDateTimeTz>()
                        .unwrap(),
                ),
                CanonicalValue::DateTimeTz(
                    "2026-01-01T00:00:00Z"
                        .parse::<CanonicalDateTimeTz>()
                        .unwrap(),
                ),
            ),
        ] {
            let group = BulkRows {
                fields: vec![field()],
                positions: vec![0],
                values: vec![vec![input]],
            };
            assert_eq!(group.correlate(&[document(output)]).unwrap(), vec![0]);
        }
    }
}
