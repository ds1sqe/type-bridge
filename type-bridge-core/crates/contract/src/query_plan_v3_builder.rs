//! Incremental construction of one `query-plan/v3` program.
//!
//! The builder is the one rule set every SDK builder calls, so the same
//! program gives the same canonical bytes in every language:
//!
//! - Binding identities are dense and follow creation order.
//! - `exact`, `subtypes`, `field`, `links`, `compare` and `let_` add to the
//!   plan's single leading `match`, in call order.
//! - Write calls append stages in call order. Consecutive calls of the same
//!   kind (`update`, `insert`, `put`, `delete`) share one stage.
//! - Derived variable names come from labels (`item`, `item_stock`), with
//!   `_2`, `_3`, ... appended on collision. Explicit names (`input`, `let_`)
//!   must be fresh.
//!
//! Every method is infallible; the first invalid call is kept and returned
//! by [`PlanBuilder::build`], which also runs full plan validation.

use super::*;
use crate::limits::MAX_OUTPUT_NAME_BYTES;
use std::sync::Arc;

/// The binding and field receiving a caller value. Use `"key"` for a
/// positional key match, or the attribute label for comparisons and writes.
#[derive(Clone, Debug)]
pub struct Site {
    /// The matched or created thing receiving the value.
    pub binding: BindingId,
    /// Attribute label, or `key` for a positional key match.
    pub field: String,
}

impl Site {
    /// Identify the binding and attribute from which an input name is derived.
    #[must_use]
    pub fn new(binding: BindingId, field: impl Into<String>) -> Self {
        Self {
            binding,
            field: field.into(),
        }
    }
}

/// One selected terminal over an immutable recording.
#[derive(Clone, Copy, Debug)]
pub enum BuildSelection<'a> {
    /// Preserve terminals explicitly recorded through the raw builder.
    Recorded,
    /// Default generated result: count reads and keep matched writes.
    Matched,
    /// Count the matched read rows.
    Count,
    /// Test whether any read row matches.
    Exists,
    /// Project existing bindings in order.
    Rows(&'a [BindingId]),
    /// Hold one model fetch, including the existing deletion-snapshot rules.
    Model(&'a ThingDocument),
}

#[cfg(test)]
std::thread_local! { static BUILDER_CLONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
#[derive(Debug, Default)]
struct CloneProbe;
#[cfg(test)]
impl Clone for CloneProbe {
    fn clone(&self) -> Self {
        BUILDER_CLONES.with(|count| count.set(count.get() + 1));
        Self
    }
}

/// One program under construction. See the module documentation for the
/// identity and naming rules.
#[derive(Clone, Debug, Default)]
pub struct PlanBuilder {
    #[cfg(test)]
    _clone_probe: CloneProbe,
    limits: QueryLimits,
    bindings: Vec<AssertionBinding>,
    types: BTreeMap<BindingId, TypeId>,
    field_values: BTreeMap<(BindingId, AttributeId), BindingId>,
    names: BTreeSet<String>,
    inputs: Vec<InputColumn>,
    rows: Vec<InputRow>,
    implicit_inputs: bool,
    bulk_inputs: bool,
    bulk_model: Option<(BindingId, Arc<Vec<BulkRows>>)>,
    patterns: Vec<Pattern>,
    writes: Vec<Stage>,
    output: Vec<BindingId>,
    read: Option<Operation>,
    returning_called: bool,
    fetch: Option<(usize, ThingDocument)>,
    error: Option<Diagnostic>,
}

fn builder_error(code: &'static str, message: &'static str) -> Diagnostic {
    invalid(code, message)
}

/// Lowercase a label into a variable spelling: other characters become
/// `_`, and a leading non-letter gains a `v` prefix.
fn variable_stem(label: &str) -> String {
    let mut stem: String = label
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '_' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '_',
        })
        .collect();
    if !stem.starts_with(|c: char| c.is_ascii_lowercase()) {
        stem.insert(0, 'v');
    }
    stem
}

impl PlanBuilder {
    /// An empty program.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record with one explicit query policy, shared by binding and validation.
    #[must_use]
    pub fn with_limits(limits: QueryLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// The policy selected when this recording was created.
    #[must_use]
    pub const fn limits(&self) -> QueryLimits {
        self.limits
    }

    /// Whether a boundary handle names a binding allocated by this recording.
    #[must_use]
    pub fn contains_binding(&self, binding: BindingId) -> bool {
        self.bindings
            .get(usize::from(binding.get()))
            .is_some_and(|known| known.id() == binding)
    }

    fn fail(&mut self, error: Diagnostic) {
        self.error.get_or_insert(error);
    }

    fn placeholder() -> BindingId {
        BindingId::new(0).expect("zero is a valid binding")
    }

    fn derived_name(&self, stem: &str) -> String {
        let mut stem = stem.to_owned();
        stem.truncate(MAX_OUTPUT_NAME_BYTES - 8);
        if !self.names.contains(&stem) {
            return stem;
        }
        (2..)
            .map(|n| format!("{stem}_{n}"))
            .find(|name| !self.names.contains(name))
            .expect("an unused suffix exists")
    }

    fn declare(&mut self, name: String) -> BindingId {
        if self.bindings.len() >= self.limits.structural.bindings {
            self.fail(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_builder_binding_limit",
                "the plan declares more bindings than its configured ceiling",
            ));
            return Self::placeholder();
        }
        let id = u16::try_from(self.bindings.len())
            .map_err(|_| ())
            .and_then(|n| BindingId::new(n).map_err(|_| ()));
        let variable = QueryVariable::new(name.clone());
        match (id, variable) {
            (Ok(id), Ok(variable)) if !self.names.contains(&name) => {
                self.names.insert(name);
                self.bindings.push(AssertionBinding::new(id, variable));
                id
            }
            (Err(()), _) => {
                self.fail(failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_plan_builder_binding_limit",
                    "the plan declares more bindings than the canonical ceiling",
                ));
                Self::placeholder()
            }
            (_, Err(error)) => {
                self.fail(error);
                Self::placeholder()
            }
            _ => {
                self.fail(builder_error(
                    "query_plan_builder_duplicate_name",
                    "a binding or input with this name already exists",
                ));
                Self::placeholder()
            }
        }
    }

    fn thing(&mut self, type_id: TypeId, include_subtypes: bool) -> BindingId {
        let name = self.derived_name(&variable_stem(type_id.label().as_str()));
        let binding = self.declare(name);
        self.types.insert(binding, type_id.clone());
        self.patterns.push(
            QueryPattern::Isa {
                binding,
                include_subtypes,
                type_id,
            }
            .into(),
        );
        binding
    }

    /// Match things of exactly `type_id`.
    pub fn exact(&mut self, type_id: TypeId) -> BindingId {
        self.thing(type_id, false)
    }

    /// Match things of `type_id` or any subtype.
    pub fn subtypes(&mut self, type_id: TypeId) -> BindingId {
        self.thing(type_id, true)
    }

    /// Restrict a matched thing to a canonical provider IID.
    pub fn by_iid(&mut self, binding: BindingId, iid: &str) -> &mut Self {
        self.patterns.push(Pattern::Iid {
            binding,
            iid: iid.to_owned(),
        });
        self
    }

    /// Restrict a matched thing to an IID supplied by one invocation column.
    /// The column can be authored as a staged result reference at finalization.
    pub fn by_iid_input(&mut self, binding: BindingId, column: InputColumnId) -> &mut Self {
        self.patterns.push(
            QueryPattern::Iid {
                binding,
                iid: QueryOperand::Input { column },
            }
            .into(),
        );
        self
    }

    /// Bind the value of `owner`'s `attribute` ownership. Owners without
    /// that ownership do not match.
    pub fn field(&mut self, owner: BindingId, attribute: AttributeId) -> BindingId {
        let owner_name = self
            .bindings
            .get(usize::from(owner.get()))
            .map_or_else(|| "v".to_owned(), |b| b.variable().as_str().to_owned());
        let stem = format!("{owner_name}_{}", variable_stem(attribute.label().as_str()));
        let name = self.derived_name(&stem);
        let value = self.declare(name);
        self.patterns.push(
            QueryPattern::Has {
                attribute: value,
                attribute_id: attribute,
                owner,
            }
            .into(),
        );
        value
    }

    /// Bind a generated handle's field on first use, reusing that binding on
    /// later accesses. Explicit [`Self::field`] calls remain independent.
    pub fn field_value(&mut self, owner: BindingId, attribute: AttributeId) -> BindingId {
        let key = (owner, attribute);
        if let Some(value) = self.field_values.get(&key) {
            return *value;
        }
        let value = self.field(owner, key.1.clone());
        self.field_values.insert(key, value);
        value
    }

    /// Require `relation` to link `player` in `role`. The relation must be
    /// a binding from [`Self::exact`] or [`Self::subtypes`].
    pub fn links(&mut self, relation: BindingId, role: RoleId, player: BindingId) -> &mut Self {
        let Some(relation_id) = self.types.get(&relation).cloned() else {
            self.fail(builder_error(
                "query_plan_builder_untyped_relation",
                "links requires a matched relation binding",
            ));
            return self;
        };
        self.patterns.push(
            QueryPattern::Links {
                players: vec![AssertionRolePlayer::new(role, player)],
                relation,
                relation_id,
            }
            .into(),
        );
        self
    }

    /// Require `left comparator right`.
    pub fn compare(
        &mut self,
        left: QueryOperand,
        comparator: ValueComparator,
        right: QueryOperand,
    ) -> &mut Self {
        self.patterns.push(
            QueryPattern::Value {
                comparator,
                left,
                right,
            }
            .into(),
        );
        self
    }

    /// Declare a typed input column named `name`; rows supply its values.
    pub fn input(&mut self, name: &str, value_type: ValueTypeTag, optional: bool) -> InputColumnId {
        if self.implicit_inputs {
            self.mixed_inputs();
            return InputColumnId::new(0);
        }
        self.input_column(name, value_type, optional)
    }

    /// Declared columns, for bindings that marshal sparse input cells.
    #[must_use]
    pub fn input_columns(&self) -> &[InputColumn] {
        &self.inputs
    }

    fn input_column(
        &mut self,
        name: &str,
        value_type: ValueTypeTag,
        optional: bool,
    ) -> InputColumnId {
        let column = InputColumnId::new(u16::try_from(self.inputs.len()).unwrap_or(u16::MAX));
        match QueryVariable::new(name) {
            Ok(_) if self.names.contains(name) => self.fail(builder_error(
                "query_plan_builder_duplicate_name",
                "a binding or input with this name already exists",
            )),
            Ok(variable) => {
                self.names.insert(name.to_owned());
                self.inputs
                    .push(InputColumn::new(column, variable, value_type, optional));
            }
            Err(error) => self.fail(error),
        }
        column
    }

    fn mixed_inputs(&mut self) {
        self.fail(builder_error(
            "plan_builder_mixed_inputs",
            "explicit, implicit scalar and bulk input forms cannot be combined",
        ));
    }

    fn site_name(&mut self, site: &Site) -> Option<String> {
        let Some(binding) = self.bindings.get(usize::from(site.binding.get())) else {
            self.fail(builder_error(
                "plan_builder_unknown_site",
                "an implicit input site must name an existing binding",
            ));
            return None;
        };
        Some(self.derived_name(&format!(
            "{}_{}",
            binding.variable().as_str(),
            variable_stem(&site.field)
        )))
    }

    /// Bind a caller value as a required typed input and return its operand.
    /// Names derive from `{binding variable}_{field}`, with collision suffixes.
    /// Explicit `input`/`row` calls cannot be mixed with implicit values.
    pub fn bind_value(&mut self, site: Site, value: CanonicalValue) -> QueryOperand {
        if self.bulk_inputs
            || (!self.implicit_inputs && (!self.inputs.is_empty() || !self.rows.is_empty()))
        {
            self.mixed_inputs();
            return QueryOperand::Input {
                column: InputColumnId::new(0),
            };
        }
        let Some(name) = self.site_name(&site) else {
            return QueryOperand::Input {
                column: InputColumnId::new(0),
            };
        };
        let column = self.input_column(&name, value.value_type(), false);
        self.implicit_inputs = true;
        if self.rows.is_empty() {
            self.rows.push(InputRow::new(vec![]));
        }
        self.rows[0].values.push(Some(value));
        QueryOperand::Input { column }
    }

    /// Bind a bulk input table, with one required typed column per site.
    /// A builder accepts one bulk table, without explicit inputs or scalar
    /// `bind_value` calls. Row arity and value types are validated at build.
    pub fn bind_rows(
        &mut self,
        columns: Vec<(Site, ValueTypeTag)>,
        rows: Vec<Vec<CanonicalValue>>,
    ) -> Vec<QueryOperand> {
        if self.implicit_inputs || !self.inputs.is_empty() || !self.rows.is_empty() {
            self.mixed_inputs();
            return vec![];
        }
        if rows.iter().any(|row| row.len() != columns.len()) {
            self.fail(builder_error(
                "plan_builder_row_arity",
                "every bulk input row must have one value per column",
            ));
            return vec![];
        }
        let mut operands = Vec::with_capacity(columns.len());
        for (site, value_type) in columns {
            let Some(name) = self.site_name(&site) else {
                return vec![];
            };
            operands.push(QueryOperand::Input {
                column: self.input_column(&name, value_type, false),
            });
        }
        self.rows = rows
            .into_iter()
            .map(|row| InputRow::new(row.into_iter().map(Some).collect()))
            .collect();
        self.implicit_inputs = true;
        self.bulk_inputs = true;
        operands
    }

    /// Record a scalar entity bulk write, grouped by present fields in core.
    /// Finalize with [`Self::build_plans`] so limits can split the groups.
    pub fn bind_model_rows(
        &mut self,
        thing: BindingId,
        rows: Vec<Vec<(OwnsFactId, CanonicalValue)>>,
    ) -> &mut Self {
        if self.implicit_inputs || !self.inputs.is_empty() || !self.rows.is_empty() {
            self.mixed_inputs();
            return self;
        }
        match group_bulk_rows(rows) {
            Ok(groups) => {
                self.bulk_model = Some((thing, Arc::new(groups)));
                self.bulk_inputs = true;
                self.implicit_inputs = true;
            }
            Err(error) => self.fail(error),
        }
        self
    }

    /// Bind bulk cells with one ownership metadata table for the whole call.
    /// Missing cells omit that ownership; grouping and split semantics are unchanged.
    pub fn bind_model_columns(
        &mut self,
        thing: BindingId,
        columns: Vec<OwnsFactId>,
        rows: Vec<Vec<Option<CanonicalValue>>>,
    ) -> &mut Self {
        if self.implicit_inputs || !self.inputs.is_empty() || !self.rows.is_empty() {
            self.mixed_inputs();
            return self;
        }
        match group_bulk_columns(columns, rows) {
            Ok(groups) => {
                self.bulk_model = Some((thing, Arc::new(groups)));
                self.bulk_inputs = true;
                self.implicit_inputs = true;
            }
            Err(error) => self.fail(error),
        }
        self
    }

    /// Bind `name` to `expr`, evaluated per matched row.
    pub fn let_(&mut self, name: &str, expr: Expr) -> BindingId {
        if self.names.contains(name) {
            self.fail(builder_error(
                "query_plan_builder_duplicate_name",
                "a binding or input with this name already exists",
            ));
            return Self::placeholder();
        }
        let binding = self.declare(name.to_owned());
        self.patterns.push(Pattern::Let { binding, expr });
        binding
    }

    /// Bind an assigned expression using the shared site naming rule.
    pub fn let_value(&mut self, site: Site, expr: Expr) -> BindingId {
        match self.site_name(&site) {
            Some(name) => self.let_(&name, expr),
            None => Self::placeholder(),
        }
    }

    fn write_stage(&mut self, same: fn(&Stage) -> bool, fresh: Stage) -> &mut Stage {
        if (matches!(fresh, Stage::Delete { .. })
            && self
                .fetch
                .as_ref()
                .is_some_and(|(after, _)| *after == self.writes.len()))
            || !self.writes.last().is_some_and(same)
        {
            self.writes.push(fresh);
        }
        self.writes
            .last_mut()
            .expect("a write stage was just ensured")
    }

    /// Replace `owner`'s `field` value with `operand`.
    pub fn update(
        &mut self,
        owner: BindingId,
        field: OwnsFactId,
        operand: QueryOperand,
    ) -> &mut Self {
        self.push_set(Set::Field {
            owner,
            field,
            operand,
        })
    }

    /// Set `relation`'s `role` player to `player`.
    pub fn update_player(
        &mut self,
        relation: BindingId,
        role: RoleId,
        player: BindingId,
    ) -> &mut Self {
        self.push_set(Set::Link {
            relation,
            role,
            player,
        })
    }

    /// Add a role player, preserving the relation's existing players.
    pub fn link(&mut self, relation: BindingId, role: RoleId, player: BindingId) -> &mut Self {
        if let Stage::Attach { items } = self.write_stage(
            |stage| matches!(stage, Stage::Attach { .. }),
            Stage::Attach { items: vec![] },
        ) {
            items.push(AttachItem::Links {
                optional: false,
                relation,
                role,
                player,
            });
        }
        self
    }

    fn push_set(&mut self, set: Set) -> &mut Self {
        if let Stage::Update { sets } = self.write_stage(
            |stage| matches!(stage, Stage::Update { .. }),
            Stage::Update { sets: vec![] },
        ) {
            sets.push(set);
        }
        self
    }

    fn new_thing(&mut self, type_id: TypeId, put: bool) -> BindingId {
        let name = self.derived_name(&variable_stem(type_id.label().as_str()));
        let binding = self.declare(name);
        self.types.insert(binding, type_id.clone());
        let thing = InsertThing {
            binding,
            type_id,
            fields: vec![],
            players: vec![],
        };
        let stage = if put {
            self.write_stage(
                |stage| matches!(stage, Stage::Put { .. }),
                Stage::Put {
                    things: vec![],
                    include_subtypes: false,
                },
            )
        } else {
            self.write_stage(
                |stage| matches!(stage, Stage::Insert { .. }),
                Stage::Insert { things: vec![] },
            )
        };
        if let Stage::Insert { things } | Stage::Put { things, .. } = stage {
            things.push(thing);
        }
        binding
    }

    /// Create a thing of `type_id`. Add its fields and players with
    /// [`Self::thing_field`] and [`Self::thing_player`].
    pub fn insert(&mut self, type_id: TypeId) -> BindingId {
        self.new_thing(type_id, false)
    }

    /// Match a thing of `type_id` with the given fields and players, or
    /// create it.
    pub fn put(&mut self, type_id: TypeId) -> BindingId {
        self.new_thing(type_id, true)
    }

    fn created(&mut self, thing: BindingId) -> Option<&mut InsertThing> {
        self.writes
            .iter_mut()
            .rev()
            .filter_map(|stage| match stage {
                Stage::Insert { things } | Stage::Put { things, .. } => Some(things),
                _ => None,
            })
            .flatten()
            .find(|candidate| candidate.binding == thing)
    }

    fn missing_thing(&mut self) {
        self.fail(builder_error(
            "query_plan_builder_not_created",
            "fields and players attach only to an inserted or put thing",
        ));
    }

    /// Give an inserted or put `thing` the `field` value `operand`.
    pub fn thing_field(
        &mut self,
        thing: BindingId,
        field: OwnsFactId,
        operand: QueryOperand,
    ) -> &mut Self {
        match self.created(thing) {
            Some(created) => created.fields.push((field, operand)),
            None => self.missing_thing(),
        }
        self
    }

    /// Give an inserted or put relation `thing` the `player` in `role`.
    pub fn thing_player(&mut self, thing: BindingId, role: RoleId, player: BindingId) -> &mut Self {
        match self.created(thing) {
            Some(created) => created.players.push((role, player)),
            None => self.missing_thing(),
        }
        self
    }

    fn push_delete(&mut self, item: DeleteItem) -> &mut Self {
        if let Stage::Delete { items } = self.write_stage(
            |stage| matches!(stage, Stage::Delete { .. }),
            Stage::Delete { items: vec![] },
        ) {
            items.push(item);
        }
        self
    }

    /// Delete `thing`.
    pub fn delete(&mut self, thing: BindingId) -> &mut Self {
        self.push_delete(DeleteItem::Thing { binding: thing })
    }

    /// Delete `owner`'s `field` ownership, keeping the owner.
    pub fn delete_field(&mut self, owner: BindingId, field: OwnsFactId) -> &mut Self {
        self.push_delete(DeleteItem::Has { owner, field })
    }

    /// Remove `player` from `relation`'s `role`.
    pub fn delete_player(
        &mut self,
        relation: BindingId,
        role: RoleId,
        player: BindingId,
    ) -> &mut Self {
        self.push_delete(DeleteItem::Links {
            relation,
            role,
            player,
        })
    }

    /// Add one input row, positional by input column.
    pub fn row(&mut self, values: Vec<Option<CanonicalValue>>) -> &mut Self {
        if self.implicit_inputs {
            self.mixed_inputs();
            return self;
        }
        self.rows.push(InputRow::new(values));
        self
    }

    /// Project `columns`, in order. A write plan then answers rows from
    /// the same pipeline instead of the matched count.
    pub fn returning(&mut self, columns: Vec<BindingId>) -> &mut Self {
        self.check_fetch_conflict();
        self.returning_called = true;
        self.output = columns;
        self
    }

    fn check_fetch_conflict(&mut self) {
        if self.fetch.is_some() {
            self.fail(builder_error(
                "query_plan_builder_terminal_conflict",
                "fetch cannot be combined with returning, count or exists",
            ));
        }
    }

    /// Answer documents describing `thing` and the players of the listed roles.
    /// A subsequent delete snapshots the thing before deleting it.
    pub fn fetch(&mut self, thing: BindingId, roles: Vec<DocumentRole>) -> &mut Self {
        if self.fetch.is_some() {
            self.fail(builder_error(
                "query_plan_builder_fetch_once",
                "a plan can fetch only once",
            ));
        } else if self.returning_called || self.read.is_some() {
            self.fail(builder_error(
                "query_plan_builder_terminal_conflict",
                "fetch cannot be combined with returning, count or exists",
            ));
        } else {
            self.fetch = Some((self.writes.len(), ThingDocument::new(thing, roles)));
        }
        self
    }

    /// Select a held write's documents after the surrounding program is
    /// recorded. A later deletion snapshots this binding before removing it.
    pub fn fetch_held(&mut self, thing: BindingId, roles: Vec<DocumentRole>) -> &mut Self {
        if self.fetch.is_some() || self.returning_called || self.read.is_some() {
            return self.fetch(thing, roles);
        }
        self.fetch = Some((0, ThingDocument::new(thing, roles)));
        self
    }

    /// Answer the number of matched rows of a read plan.
    pub fn count(&mut self) -> &mut Self {
        self.check_fetch_conflict();
        self.read = Some(Operation::Count);
        self
    }

    /// Answer whether a read plan matches any row.
    pub fn exists(&mut self) -> &mut Self {
        self.check_fetch_conflict();
        self.read = Some(Operation::Exists);
        self
    }

    /// Validate the program against `schema`, under `semantics`.
    ///
    /// A plan with write stages is a write: it answers the matched count,
    /// or rows when [`Self::returning`] named columns or [`Self::fetch`] requests documents. A read answers rows,
    /// or [`Self::count`] / [`Self::exists`].
    ///
    /// # Errors
    ///
    /// Returns the first invalid builder call, a write plan that also asked
    /// for count or exists, or any plan validation failure.
    pub fn build(
        &self,
        schema: &DeclaredSchema,
        semantics: ManagedSemanticSchemaFingerprint,
    ) -> Result<QueryPlanV3, Diagnostic> {
        self.build_selected(schema, semantics, BuildSelection::Recorded)
    }

    /// Build one staged entry through the ordinary plan validator.
    ///
    /// Reference columns contain `None` in the invocation template; their
    /// future IID values are bound only by the staged transaction executor.
    /// The complete stack validates source order and type compatibility.
    pub fn build_staged(
        &self,
        schema: &DeclaredSchema,
        semantics: ManagedSemanticSchemaFingerprint,
        rows: Vec<InputRow>,
        result_refs: Vec<crate::query_remote_v3::StagedResultRefV3>,
    ) -> Result<crate::query_remote_v3::StagedPlanV3, Diagnostic> {
        let plan = self.build(schema, semantics)?;
        crate::query_remote_v3::StagedPlanV3::new_with_result_refs(&plan, rows, result_refs, schema)
    }

    /// Build one selected terminal without copying the recording first.
    /// Bulk programs use [`Self::build_selected_plans`] instead.
    pub fn build_selected(
        &self,
        schema: &DeclaredSchema,
        semantics: ManagedSemanticSchemaFingerprint,
        selection: BuildSelection<'_>,
    ) -> Result<QueryPlanV3, Diagnostic> {
        // The plan stage; spans carry no fields, so no user value leaves.
        let _stage = tracing::info_span!("tb.orm.plan").entered();
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.bulk_model.is_some() {
            return Err(builder_error(
                "plan_builder_bulk_split_required",
                "finalize model bulk writes with build_plans",
            ));
        }
        let (read, fetch) = match selection {
            BuildSelection::Recorded => (
                self.read,
                self.fetch
                    .as_ref()
                    .map(|(after, document)| (*after, document)),
            ),
            BuildSelection::Matched => {
                if self.writes.is_empty() && self.fetch.is_some() {
                    return Err(builder_error(
                        "query_plan_builder_terminal_conflict",
                        "fetch cannot be combined with returning, count or exists",
                    ));
                }
                (
                    if self.writes.is_empty() {
                        Some(Operation::Count)
                    } else {
                        self.read
                    },
                    self.fetch
                        .as_ref()
                        .map(|(after, document)| (*after, document)),
                )
            }
            BuildSelection::Count | BuildSelection::Exists | BuildSelection::Rows(_) => {
                if self.fetch.is_some() {
                    return Err(builder_error(
                        "query_plan_builder_terminal_conflict",
                        "fetch cannot be combined with returning, count or exists",
                    ));
                }
                (
                    match selection {
                        BuildSelection::Count => Some(Operation::Count),
                        BuildSelection::Exists => Some(Operation::Exists),
                        _ => self.read,
                    },
                    None,
                )
            }
            BuildSelection::Model(document) => {
                if self.fetch.is_some() {
                    return Err(builder_error(
                        "query_plan_builder_fetch_once",
                        "a plan can fetch only once",
                    ));
                }
                if self.returning_called || self.read.is_some() {
                    return Err(builder_error(
                        "query_plan_builder_terminal_conflict",
                        "fetch cannot be combined with returning, count or exists",
                    ));
                }
                (None, Some((0, document)))
            }
        };
        let columns = match selection {
            BuildSelection::Rows(columns) => columns,
            _ => &self.output,
        };
        let operation = match (self.writes.is_empty(), read) {
            (true, read) => read.unwrap_or(Operation::Rows),
            (false, None) => Operation::Write {
                returns: if columns.is_empty() && fetch.is_none() {
                    WriteReturns::Matched
                } else {
                    WriteReturns::Rows
                },
            },
            (false, Some(_)) => {
                return Err(builder_error(
                    "query_plan_builder_write_operation",
                    "a write plan answers matched rows or projected rows",
                ));
            }
        };
        let mut pipeline = Vec::with_capacity(self.writes.len() + 1);
        if !self.patterns.is_empty() {
            pipeline.push(Stage::Match {
                patterns: self.patterns.clone(),
            });
        }
        pipeline.extend(self.writes.iter().cloned());
        if let Some((after, document)) = fetch {
            let deletion = self.writes[after..].iter().position(|stage| {
                matches!(stage, Stage::Delete { items } if items.iter().any(|item| {
                    matches!(item, DeleteItem::Thing { binding } if *binding == document.thing)
                }))
            });
            let position = deletion.map_or(pipeline.len(), |offset| {
                after + offset + usize::from(!self.patterns.is_empty())
            });
            pipeline.insert(position, Stage::Fetch(document.clone()));
        }
        QueryPlanV3::new_with_limits(
            PlanSpec {
                bindings: self.bindings.clone(),
                inputs: self.inputs.clone(),
                given_rows: (self.implicit_inputs || !self.rows.is_empty())
                    .then(|| self.rows.clone()),
                operation,
                pipeline,
                output: QueryOutput::Rows {
                    columns: if matches!(operation, Operation::Count | Operation::Exists) {
                        vec![]
                    } else {
                        columns.to_vec()
                    },
                },
                managed_semantics: semantics,
                functions: vec![],
            },
            schema,
            self.limits,
        ).inspect(|plan| {
            if let Ok(fingerprint) = plan.shape_fingerprint() {
                tracing::debug!(target: "type_bridge.plan", origin = "builder", fingerprint = %fingerprint.digest().to_hex(), operation = ?plan.operation(), stages = plan.pipeline().len(), statements = plan.statement_counts().0 + plan.statement_counts().1, writes = plan.writes(), "type_bridge.plan.build");
            }
        })
    }

    /// Build the generated context's default count-only result. Writes keep
    /// their matched answer; reads count rows without projecting bindings.
    ///
    /// # Errors
    /// Returns the same construction and validation errors as [`Self::build`].
    pub fn build_matched(
        &self,
        schema: &DeclaredSchema,
        semantics: ManagedSemanticSchemaFingerprint,
    ) -> Result<QueryPlanV3, Diagnostic> {
        self.build_selected(schema, semantics, BuildSelection::Matched)
    }

    /// Build a generated program, splitting scalar entity bulk rows only when
    /// their field shape or the supplied row/byte limits require it. All
    /// returned plans belong to the same transaction.
    ///
    /// # Errors
    /// Refuses mixed programs, list ownerships, unsplittable rows, or ordinary
    /// plan construction/validation failures.
    pub fn build_plans(
        &self,
        schema: &DeclaredSchema,
        semantics: ManagedSemanticSchemaFingerprint,
        matched: bool,
        limits: crate::split::SplitLimits,
    ) -> Result<BuiltPlans, Diagnostic> {
        self.build_selected_plans(
            schema,
            semantics,
            if matched {
                BuildSelection::Matched
            } else {
                BuildSelection::Recorded
            },
            limits,
        )
    }

    /// Select a terminal and split bulk rows through the same borrowed recording.
    pub fn build_selected_plans(
        &self,
        schema: &DeclaredSchema,
        semantics: ManagedSemanticSchemaFingerprint,
        selection: BuildSelection<'_>,
        limits: crate::split::SplitLimits,
    ) -> Result<BuiltPlans, Diagnostic> {
        let finish = |builder: &Self| builder.build_selected(schema, semantics.clone(), selection);
        let Some((thing, groups)) = &self.bulk_model else {
            return finish(self).map(BuiltPlans::single);
        };
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        // shortcut: bulk writes are standalone entity stages; extend for a specified compound bulk program.
        if !self.patterns.is_empty()
            || self.writes.len() != 1
            || !matches!(&self.writes[0], Stage::Insert { things } | Stage::Put { things, .. } if things.len() == 1 && things[0].binding == *thing && things[0].type_id.kind() == TypeKind::Entity && things[0].fields.is_empty() && things[0].players.is_empty())
        {
            return Err(builder_error(
                "plan_builder_mixed_inputs",
                "bulk rows require one standalone entity insert or put",
            ));
        }
        let mut base = self.clone();
        base.bulk_model = None;
        base.bulk_inputs = false;
        base.implicit_inputs = false;
        let empty = finish(&base)?;
        let documents = empty.fetch_document_shape().is_some();
        if !documents
            && !matches!(
                empty.operation(),
                Operation::Write {
                    returns: WriteReturns::Matched
                }
            )
        {
            return Err(builder_error(
                "plan_builder_bulk_projection",
                "model bulk writes answer matched counts or fetched documents",
            ));
        }
        let type_id = &self.types[thing];
        let profile = crate::semantic_profile::SemanticProfile::resolve(
            semantics
                .as_fingerprint()
                .semantic_profile()
                .expect("managed semantics has a profile"),
        )?;
        if SchemaIndex::new(schema).has_list_ownership(type_id, &profile) {
            return Err(builder_error(
                "plan_builder_bulk_scalar_ownerships",
                "generated bulk writes require scalar ownerships",
            ));
        }
        let mut plans = Vec::new();
        let mut chunks = Vec::new();
        for (group_index, group) in groups.iter().enumerate() {
            if group.fields.is_empty() {
                if limits.max_rows == 0 || empty.canonical_bytes()?.len() > limits.max_bytes {
                    return Err(builder_error(
                        "query_invocation_input_byte_limit",
                        "bulk input cannot fit the plan row/byte limits",
                    ));
                }
                for position in 0..group.positions.len() {
                    plans.push(empty.clone());
                    chunks.push(BulkChunk {
                        group: group_index,
                        range: position..position + 1,
                    });
                }
                continue;
            }
            let mut builder = base.clone();
            let mut columns = Vec::new();
            for (field, value) in group.fields.iter().zip(&group.values[0]) {
                let name = builder
                    .site_name(&Site::new(*thing, field.attribute().label().as_str()))
                    .expect("validated bulk binding exists");
                let column = builder.input(&name, value.value_type(), false);
                columns.push((field.clone(), QueryOperand::Input { column }));
            }
            builder
                .created(*thing)
                .expect("validated created thing exists")
                .fields = columns;
            let template = finish(&builder)?;
            let plan_bytes = template.canonical_bytes()?.len() + b",\"given_rows\":[]".len() - 1;
            let units = group
                .values
                .iter()
                .map(|row| {
                    serde_json::to_vec(&InputRow::new(row.iter().cloned().map(Some).collect()))
                        .map(|bytes| crate::split::SplitUnit {
                            rows: 1,
                            bytes: bytes.len() + 1,
                        })
                        .map_err(|_| {
                            builder_error(
                                "query_invocation_inputs_unencodable",
                                "bulk row cannot be encoded",
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let limits = crate::split::SplitLimits::new(
                limits
                    .max_rows
                    .min(crate::limits::MAX_INPUT_ROWS)
                    .min(self.limits.structural.input_rows),
                limits
                    .max_bytes
                    .min(crate::limits::MAX_REMOTE_ENVELOPE_BYTES)
                    .min(self.limits.codec.max_bytes)
                    .min(plan_bytes.saturating_add(crate::limits::MAX_INPUT_BYTES - 1))
                    .min(
                        plan_bytes
                            .saturating_add(self.limits.structural.input_bytes.saturating_sub(1)),
                    ),
            );
            let ranges = crate::split::split(plan_bytes, &units, limits).map_err(|_| {
                builder_error(
                    "query_invocation_input_byte_limit",
                    "bulk input cannot fit the plan row/byte limits",
                )
            })?;
            for range in ranges {
                let rows = group.values[range.clone()]
                    .iter()
                    .map(|row| InputRow::new(row.iter().cloned().map(Some).collect()))
                    .collect();
                plans.push(template.with_given_rows(rows, schema)?);
                chunks.push(BulkChunk {
                    group: group_index,
                    range,
                });
            }
        }
        Ok(BuiltPlans::bulk(plans, Arc::clone(groups), chunks, empty))
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    fn fixture() -> (DeclaredSchema, ManagedSemanticSchemaFingerprint, TypeId) {
        use crate::schema::*;
        let entity = TypeId::new(TypeKind::Entity, "other").unwrap();
        let span =
            SourceSpan::new(DocumentId::new("selection.yaml").unwrap(), 0, 1, 1, 1, 1, 2).unwrap();
        let schema = DeclaredSchema::from_facts(
            crate::codec::FormatVersion::V1,
            crate::capability::CapabilitySet::new(),
            [SourcedSchemaFact::new(
                SchemaFact::Type(TypeFact::new(entity.clone()).unwrap()),
                span,
            )],
        )
        .unwrap();
        let semantics = ManagedSemanticSchemaFingerprint::compute(
            crate::fingerprint::SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
            b"selection-test",
        )
        .unwrap();
        (schema, semantics, entity)
    }
    #[test]
    fn scalar_metadata_is_optional_and_does_not_change_canonical_bytes() {
        let (schema, semantics, entity) = fixture();
        let mut builder = PlanBuilder::new();
        let thing = builder.exact(entity);
        let value = builder.let_(
            "number",
            Expr::Operand(QueryOperand::Literal {
                value: CanonicalValue::Long(7),
            }),
        );
        let counted = builder.build_matched(&schema, semantics.clone()).unwrap();
        assert!(counted.scalar_types.is_none());
        let modeled = builder
            .build_selected(
                &schema,
                semantics.clone(),
                BuildSelection::Model(&ThingDocument::new(thing, vec![])),
            )
            .unwrap();
        assert!(modeled.scalar_types.is_none());
        builder.returning(vec![thing, value]);
        let selected = builder.build(&schema, semantics).unwrap();
        assert_eq!(selected.scalar_value_type(value), Some(ValueTypeTag::Long));
        assert_eq!(selected.scalar_types.as_ref().unwrap().len(), 1);
        let bytes = selected.canonical_bytes().unwrap();
        let mut without_metadata = selected.clone();
        without_metadata.scalar_types = None;
        assert_eq!(without_metadata.canonical_bytes().unwrap(), bytes);
        let decoded = super::super::decode_query_plan_v3(&bytes, &schema).unwrap();
        assert_eq!(decoded.canonical_bytes().unwrap(), bytes);
        assert_eq!(decoded.scalar_value_type(value), Some(ValueTypeTag::Long));
        let counted_bytes = counted.canonical_bytes().unwrap();
        let decoded = super::super::decode_query_plan_v3(&counted_bytes, &schema).unwrap();
        assert!(decoded.scalar_types.is_none());
        assert_eq!(decoded.canonical_bytes().unwrap(), counted_bytes);
    }

    #[test]
    fn matched_selection_never_clones_the_recorder() {
        let (schema, semantics, entity) = fixture();
        let mut builder = PlanBuilder::new();
        builder.exact(entity);
        let before = BUILDER_CLONES.with(std::cell::Cell::get);
        let plan = builder.build_matched(&schema, semantics).unwrap();
        assert!(matches!(plan.operation(), Operation::Count));
        assert_eq!(BUILDER_CLONES.with(std::cell::Cell::get), before);
    }
    #[test]
    fn bulk_builder_copies_and_built_chunks_share_the_original_rows() {
        let (schema, semantics, entity) = fixture();
        let mut builder = PlanBuilder::new();
        let thing = builder.insert(entity);
        builder.bind_model_rows(thing, vec![vec![], vec![]]);
        let copied = builder.clone();
        let source = &builder.bulk_model.as_ref().unwrap().1;
        assert!(Arc::ptr_eq(source, &copied.bulk_model.as_ref().unwrap().1));
        let built = builder
            .build_plans(
                &schema,
                semantics,
                false,
                crate::split::SplitLimits::CANONICAL,
            )
            .unwrap();
        assert!(Arc::ptr_eq(source, built.bulk_source().unwrap()));
        let chunk = built.bulk_chunks().unwrap().next().unwrap();
        assert!(std::ptr::eq(
            chunk.values.as_ptr(),
            source[0].values.as_ptr()
        ));
    }
    #[test]
    fn borrowed_model_selection_matches_recorded_fetch_and_does_not_clone_the_recorder() {
        let (schema, semantics, entity) = fixture();
        let mut builder = PlanBuilder::new();
        let thing = builder.exact(entity);
        let document = ThingDocument::new(thing, vec![]);
        let before = BUILDER_CLONES.with(std::cell::Cell::get);
        let selected = builder
            .build_selected(&schema, semantics.clone(), BuildSelection::Model(&document))
            .unwrap();
        assert_eq!(BUILDER_CLONES.with(std::cell::Cell::get), before);
        builder.fetch_held(thing, vec![]);
        assert_eq!(
            selected.canonical_bytes().unwrap(),
            builder
                .build(&schema, semantics)
                .unwrap()
                .canonical_bytes()
                .unwrap()
        );
    }
}
