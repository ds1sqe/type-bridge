//! Provider-neutral fact dependency planning for schema deltas.

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationSubjectId, DeclaredSchema, RelatesFactId, SchemaFact, SchemaFactId, SchemaOperation,
    SubFactId, ValueFactId,
};

/// The exact fact-level dependency graph used to order schema operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactDependencyGraph {
    dependencies: BTreeMap<SchemaFactId, BTreeSet<SchemaFactId>>,
    dependents: BTreeMap<SchemaFactId, BTreeSet<SchemaFactId>>,
}

impl FactDependencyGraph {
    /// Build the graph from a complete declared fact inventory.
    pub fn from_declared(declared: &DeclaredSchema) -> Result<Self, Diagnostic> {
        Self::from_facts(declared.facts())
    }

    /// Build the graph from a complete fact inventory.
    pub fn from_facts<'a>(
        facts: impl IntoIterator<Item = &'a SchemaFact>,
    ) -> Result<Self, Diagnostic> {
        let mut inventory = BTreeMap::new();
        for fact in facts {
            let id = fact.id();
            if inventory.insert(id.clone(), fact.clone()).is_some() {
                return Err(failure(
                    "schema_delta_duplicate_fact",
                    format!("duplicate fact identity in dependency inventory: {id:?}"),
                ));
            }
        }

        let mut labels: BTreeMap<Label, Vec<SchemaFactId>> = BTreeMap::new();
        let mut sub_by_child: BTreeMap<TypeId, Vec<SubFactId>> = BTreeMap::new();
        for fact in inventory.values() {
            match fact {
                SchemaFact::Type(fact) => labels
                    .entry(fact.id().label().clone())
                    .or_default()
                    .push(SchemaFactId::Type(fact.id().clone())),
                SchemaFact::Struct(fact) => labels
                    .entry(fact.id().label().clone())
                    .or_default()
                    .push(SchemaFactId::Struct(fact.id().clone())),
                SchemaFact::Sub(fact) => sub_by_child
                    .entry(fact.id().subtype().clone())
                    .or_default()
                    .push(fact.id().clone()),
                _ => {}
            }
        }
        for candidates in labels.values_mut() {
            candidates.sort();
        }
        for edges in sub_by_child.values_mut() {
            edges.sort();
        }

        let mut dependencies = BTreeMap::new();
        for fact in inventory.values() {
            dependencies.insert(fact.id(), dependencies_for(fact, &labels, &sub_by_child)?);
        }

        let mut dependents: BTreeMap<SchemaFactId, BTreeSet<SchemaFactId>> = BTreeMap::new();
        for id in dependencies.keys() {
            dependents.entry(id.clone()).or_default();
        }
        for (dependent, prerequisites) in &dependencies {
            for prerequisite in prerequisites {
                dependents
                    .entry(prerequisite.clone())
                    .or_default()
                    .insert(dependent.clone());
            }
        }

        Ok(Self {
            dependencies,
            dependents,
        })
    }

    /// Return the direct prerequisites of one fact.
    #[must_use]
    pub fn dependencies(&self, id: &SchemaFactId) -> Option<&BTreeSet<SchemaFactId>> {
        self.dependencies.get(id)
    }

    /// Return the direct dependents of one fact.
    #[must_use]
    pub fn dependents(&self, id: &SchemaFactId) -> Option<&BTreeSet<SchemaFactId>> {
        self.dependents.get(id)
    }

    /// Reject an inventory with a dependency outside the inventory.
    pub fn validate_complete(&self) -> Result<(), Diagnostic> {
        for (dependent, prerequisites) in &self.dependencies {
            for prerequisite in prerequisites {
                if !self.dependencies.contains_key(prerequisite) {
                    return Err(failure(
                        "schema_delta_missing_dependency",
                        format!("fact {dependent:?} requires absent fact {prerequisite:?}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Compute the exact formal diff and deterministic dependency-safe operation order.
pub fn plan_schema_operations(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
) -> Result<Vec<SchemaOperation>, Diagnostic> {
    let source_facts = fact_map(source);
    let target_facts = fact_map(target);
    let source_graph = FactDependencyGraph::from_declared(source)?;
    let target_graph = FactDependencyGraph::from_declared(target)?;
    source_graph.validate_complete()?;
    target_graph.validate_complete()?;

    let source_ids: BTreeSet<_> = source_facts.keys().cloned().collect();
    let target_ids: BTreeSet<_> = target_facts.keys().cloned().collect();
    let added: BTreeSet<_> = target_ids.difference(&source_ids).cloned().collect();
    let removed: BTreeSet<_> = source_ids.difference(&target_ids).cloned().collect();
    let redefined: BTreeSet<_> = source_ids
        .intersection(&target_ids)
        .filter(|id| source_facts.get(*id) != target_facts.get(*id))
        .cloned()
        .collect();

    for id in added.iter().chain(removed.iter()).chain(redefined.iter()) {
        if matches!(
            source_facts.get(id).or_else(|| target_facts.get(id)),
            Some(SchemaFact::Function(_))
        ) {
            return Err(failure(
                "unsupported_function_migration",
                "automatic migration of opaque function bodies is unsupported",
            ));
        }
    }

    let mut operations = Vec::new();

    // Establish all new identities before replacements can refer to them.
    for component in ordered_components(&target_graph, &added) {
        let facts = component
            .into_iter()
            .map(|id| target_facts[&id].clone())
            .collect();
        operations.push(SchemaOperation::define(facts)?);
    }

    // Replacements retain their identity, so SCC members can be emitted in stable ID order.
    for component in ordered_components(&target_graph, &redefined) {
        for id in component {
            operations.push(SchemaOperation::redefine(
                source_facts[&id].clone(),
                target_facts[&id].clone(),
            )?);
        }
    }

    // Dependents must disappear before their prerequisites.
    for component in ordered_components(&source_graph, &removed)
        .into_iter()
        .rev()
    {
        for id in component.into_iter().rev() {
            operations.push(SchemaOperation::undefine(source_facts[&id].clone()));
        }
    }

    Ok(operations)
}

fn fact_map(declared: &DeclaredSchema) -> BTreeMap<SchemaFactId, SchemaFact> {
    declared
        .facts()
        .map(|fact| (fact.id(), fact.clone()))
        .collect()
}

fn dependencies_for(
    fact: &SchemaFact,
    labels: &BTreeMap<Label, Vec<SchemaFactId>>,
    sub_by_child: &BTreeMap<TypeId, Vec<SubFactId>>,
) -> Result<BTreeSet<SchemaFactId>, Diagnostic> {
    let mut dependencies = BTreeSet::new();
    match fact {
        SchemaFact::Type(_) | SchemaFact::Struct(_) => {}
        SchemaFact::Sub(fact) => {
            dependencies.insert(SchemaFactId::Type(fact.id().subtype().clone()));
            dependencies.insert(SchemaFactId::Type(fact.id().supertype().clone()));
        }
        SchemaFact::Value(fact) => {
            dependencies.insert(SchemaFactId::Type(attribute_type(fact.id().attribute())?));
        }
        SchemaFact::Owns(fact) => {
            dependencies.insert(SchemaFactId::Type(fact.id().owner().clone()));
            dependencies.insert(SchemaFactId::Type(attribute_type(fact.id().attribute())?));
            dependencies.insert(SchemaFactId::Value(ValueFactId::new(
                fact.id().attribute().clone(),
            )));
        }
        SchemaFact::Relates(fact) => {
            dependencies.insert(SchemaFactId::Type(fact.id().relation().clone()));
            if let Some(parent_role) = fact.specializes() {
                dependencies.insert(SchemaFactId::Relates(relates_id(parent_role)?));
                let parent_relation = declaring_relation_type(parent_role)?;
                let path = find_sub_path(
                    fact.id().relation(),
                    &parent_relation,
                    sub_by_child,
                    &mut BTreeSet::new(),
                )
                .ok_or_else(|| {
                    failure(
                        "schema_delta_missing_specialization_path",
                        format!(
                            "role specialization has no subtype path from {:?} to {parent_relation:?}",
                            fact.id().relation()
                        ),
                    )
                })?;
                dependencies.extend(path.into_iter().map(SchemaFactId::Sub));
            }
        }
        SchemaFact::Plays(fact) => {
            dependencies.insert(SchemaFactId::Type(fact.id().player().clone()));
            dependencies.insert(SchemaFactId::Relates(relates_id(fact.id().role())?));
        }
        SchemaFact::Annotation(fact) => {
            dependencies.insert(annotation_subject_id(fact.id().subject()));
        }
        SchemaFact::Function(fact) => {
            for label in fact.schema_references() {
                let candidates = labels.get(label).ok_or_else(|| {
                    failure(
                        "schema_delta_missing_function_reference",
                        format!("function references absent schema label {label:?}"),
                    )
                })?;
                if candidates.len() != 1 {
                    return Err(failure(
                        "schema_delta_ambiguous_function_reference",
                        format!("function schema label {label:?} is ambiguous"),
                    ));
                }
                dependencies.insert(candidates[0].clone());
            }
        }
    }
    Ok(dependencies)
}

fn annotation_subject_id(subject: &AnnotationSubjectId) -> SchemaFactId {
    match subject {
        AnnotationSubjectId::Type(id) => SchemaFactId::Type(id.clone()),
        AnnotationSubjectId::Sub(id) => SchemaFactId::Sub(id.clone()),
        AnnotationSubjectId::Value(id) => SchemaFactId::Value(id.clone()),
        AnnotationSubjectId::Owns(id) => SchemaFactId::Owns(id.clone()),
        AnnotationSubjectId::Relates(id) => SchemaFactId::Relates(id.clone()),
        AnnotationSubjectId::Plays(id) => SchemaFactId::Plays(id.clone()),
        AnnotationSubjectId::Function(id) => SchemaFactId::Function(id.clone()),
    }
}

fn attribute_type(attribute: &type_bridge_contract::id::AttributeId) -> Result<TypeId, Diagnostic> {
    TypeId::new(TypeKind::Attribute, attribute.label().as_str())
}

fn declaring_relation_type(role: &RoleId) -> Result<TypeId, Diagnostic> {
    TypeId::new(TypeKind::Relation, role.declaring_relation().as_str())
}

fn relates_id(role: &RoleId) -> Result<RelatesFactId, Diagnostic> {
    RelatesFactId::new(declaring_relation_type(role)?, role.clone())
}

fn find_sub_path(
    current: &TypeId,
    target: &TypeId,
    sub_by_child: &BTreeMap<TypeId, Vec<SubFactId>>,
    visited: &mut BTreeSet<TypeId>,
) -> Option<Vec<SubFactId>> {
    if current == target {
        return Some(Vec::new());
    }
    if !visited.insert(current.clone()) {
        return None;
    }
    for edge in sub_by_child.get(current).into_iter().flatten() {
        if let Some(mut tail) = find_sub_path(edge.supertype(), target, sub_by_child, visited) {
            let mut path = vec![edge.clone()];
            path.append(&mut tail);
            return Some(path);
        }
    }
    None
}

fn ordered_components(
    graph: &FactDependencyGraph,
    nodes: &BTreeSet<SchemaFactId>,
) -> Vec<Vec<SchemaFactId>> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let components = strongly_connected_components(graph, nodes);
    let mut component_of = BTreeMap::new();
    for (index, component) in components.iter().enumerate() {
        for node in component {
            component_of.insert(node.clone(), index);
        }
    }

    let mut prerequisites = vec![BTreeSet::new(); components.len()];
    let mut dependents = vec![BTreeSet::new(); components.len()];
    for (index, component) in components.iter().enumerate() {
        for node in component {
            for dependency in graph.dependencies(node).into_iter().flatten() {
                if let Some(dependency_index) = component_of.get(dependency).copied()
                    && dependency_index != index
                {
                    prerequisites[index].insert(dependency_index);
                    dependents[dependency_index].insert(index);
                }
            }
        }
    }

    let keys: Vec<_> = components
        .iter()
        .map(|component| component[0].clone())
        .collect();
    let mut remaining: Vec<_> = prerequisites.iter().map(BTreeSet::len).collect();
    let mut ready = BTreeSet::new();
    for (index, count) in remaining.iter().enumerate() {
        if *count == 0 {
            ready.insert((keys[index].clone(), index));
        }
    }

    let mut ordered = Vec::with_capacity(components.len());
    while let Some(entry) = ready.iter().next().cloned() {
        ready.remove(&entry);
        let index = entry.1;
        ordered.push(components[index].clone());
        for dependent in dependents[index].iter().copied() {
            remaining[dependent] -= 1;
            if remaining[dependent] == 0 {
                ready.insert((keys[dependent].clone(), dependent));
            }
        }
    }
    ordered
}

fn strongly_connected_components(
    graph: &FactDependencyGraph,
    nodes: &BTreeSet<SchemaFactId>,
) -> Vec<Vec<SchemaFactId>> {
    struct Tarjan {
        next_index: usize,
        stack: Vec<SchemaFactId>,
        on_stack: BTreeSet<SchemaFactId>,
        indices: BTreeMap<SchemaFactId, usize>,
        lowlinks: BTreeMap<SchemaFactId, usize>,
        components: Vec<Vec<SchemaFactId>>,
    }

    fn visit(
        node: SchemaFactId,
        graph: &FactDependencyGraph,
        nodes: &BTreeSet<SchemaFactId>,
        state: &mut Tarjan,
    ) {
        let index = state.next_index;
        state.next_index += 1;
        state.indices.insert(node.clone(), index);
        state.lowlinks.insert(node.clone(), index);
        state.stack.push(node.clone());
        state.on_stack.insert(node.clone());

        let neighbors: Vec<_> = graph
            .dependencies(&node)
            .into_iter()
            .flatten()
            .filter(|neighbor| nodes.contains(*neighbor))
            .cloned()
            .collect();
        for neighbor in neighbors {
            if !state.indices.contains_key(&neighbor) {
                visit(neighbor.clone(), graph, nodes, state);
                let neighbor_lowlink = state.lowlinks[&neighbor];
                let lowlink = state.lowlinks.get_mut(&node).expect("visited node");
                *lowlink = (*lowlink).min(neighbor_lowlink);
            } else if state.on_stack.contains(&neighbor) {
                let neighbor_index = state.indices[&neighbor];
                let lowlink = state.lowlinks.get_mut(&node).expect("visited node");
                *lowlink = (*lowlink).min(neighbor_index);
            }
        }

        if state.lowlinks[&node] == state.indices[&node] {
            let mut component = Vec::new();
            loop {
                let member = state.stack.pop().expect("SCC root is on stack");
                state.on_stack.remove(&member);
                component.push(member.clone());
                if member == node {
                    break;
                }
            }
            component.sort();
            state.components.push(component);
        }
    }

    let mut state = Tarjan {
        next_index: 0,
        stack: Vec::new(),
        on_stack: BTreeSet::new(),
        indices: BTreeMap::new(),
        lowlinks: BTreeMap::new(),
        components: Vec::new(),
    };
    for node in nodes {
        if !state.indices.contains_key(node) {
            visit(node.clone(), graph, nodes, &mut state);
        }
    }
    state.components
}

fn failure(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("static diagnostic code is canonical"),
        message,
    )
}
