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

/// One program under construction. See the module documentation for the
/// identity and naming rules.
#[derive(Clone, Debug, Default)]
pub struct PlanBuilder {
    bindings: Vec<AssertionBinding>,
    types: BTreeMap<BindingId, TypeId>,
    names: BTreeSet<String>,
    inputs: Vec<InputColumn>,
    rows: Vec<InputRow>,
    patterns: Vec<Pattern>,
    writes: Vec<Stage>,
    output: Vec<BindingId>,
    read: Option<Operation>,
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

    fn write_stage(&mut self, same: fn(&Stage) -> bool, fresh: Stage) -> &mut Stage {
        if !self.writes.last().is_some_and(same) {
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
                Stage::Put { things: vec![] },
            )
        } else {
            self.write_stage(
                |stage| matches!(stage, Stage::Insert { .. }),
                Stage::Insert { things: vec![] },
            )
        };
        if let Stage::Insert { things } | Stage::Put { things } = stage {
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
                Stage::Insert { things } | Stage::Put { things } => Some(things),
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
        self.rows.push(InputRow::new(values));
        self
    }

    /// Project `columns`, in order. A write plan then answers rows from
    /// the same pipeline instead of the matched count.
    pub fn returning(&mut self, columns: Vec<BindingId>) -> &mut Self {
        self.output = columns;
        self
    }

    /// Answer the number of matched rows of a read plan.
    pub fn count(&mut self) -> &mut Self {
        self.read = Some(Operation::Count);
        self
    }

    /// Answer whether a read plan matches any row.
    pub fn exists(&mut self) -> &mut Self {
        self.read = Some(Operation::Exists);
        self
    }

    /// Validate the program against `schema`, under `semantics`.
    ///
    /// A plan with write stages is a write: it answers the matched count,
    /// or rows when [`Self::returning`] named columns. A read answers rows,
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
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let operation = match (self.writes.is_empty(), self.read) {
            (true, read) => read.unwrap_or(Operation::Rows),
            (false, None) => Operation::Write {
                returns: if self.output.is_empty() {
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
        QueryPlanV3::new(
            PlanSpec {
                bindings: self.bindings.clone(),
                inputs: self.inputs.clone(),
                given_rows: (!self.rows.is_empty()).then(|| self.rows.clone()),
                operation,
                pipeline,
                output: QueryOutput::Rows {
                    columns: if matches!(operation, Operation::Count | Operation::Exists) {
                        vec![]
                    } else {
                        self.output.clone()
                    },
                },
                managed_semantics: semantics,
                compatibility: None,
                functions: vec![],
            },
            schema,
        )
    }
}
