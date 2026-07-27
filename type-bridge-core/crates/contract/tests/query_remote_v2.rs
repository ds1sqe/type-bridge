use sha2::{Digest, Sha256};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPath, DiagnosticPathSegment,
};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    CompatibilityValueV2, HydrationBindingV2, HydrationDescriptorV2, HydrationFieldV2,
    HydrationPlayerV2, HydrationProjectionV2, HydrationRoleV2, ModelQueryV2, QueryBindingPairV2,
    QueryFieldV2, QueryMissingOrderV2, QueryModelOutputSlotV2, QueryModelOutputV2, QueryOperation,
    QueryOrderDirectionV2, QueryOrderTermV2, QueryOutput, QueryPattern, QueryPlan,
    QueryPlanV2Compatibility, QueryRowCardinalityV2, QueryStableOrderV2, QueryWindowV2, ReadStage,
};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteExecutorBinding, RemoteReplySignature, RemoteReplySigner,
    RemoteReplySigningDigest, RemoteReplyVerifier, RemoteSigningPublicKey,
};
use type_bridge_contract::query_remote_v2::{
    CAP_QUERY_OUTPUT_HYDRATED, CAP_QUERY_PLAN_V2, CAP_QUERY_REMOTE_ENVELOPE_V2,
    CAP_QUERY_REMOTE_STRUCTURED_DIAGNOSTIC, CAP_QUERY_SAME_SNAPSHOT_HYDRATION, HydratedRowV2,
    HydrationAttributeEvidenceV2, HydrationGraphV2, HydrationNodeIdV2, HydrationNodeKindV2,
    HydrationNodeV2, HydrationReferenceV2, HydrationRoleEvidenceV2, HydrationSlotV2,
    QUERY_REMOTE_REQUEST_CANONICALIZATION_V2, RemoteLimitsV2, RemoteOutcomeV2,
    RemoteQueryFailureV2, RemoteQueryRequestV2, RemoteQueryResponseV2, RemoteReplyDecodeLimitsV2,
    RemoteReplyV2, RemoteResultKindV2, decode_remote_reply_v2, decode_signed_remote_failure_v2,
    query_remote_v2_required_capabilities, validate_remote_outcome_v2,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{
    CanonicalString, CanonicalValue, Cardinality, DecimalValue, ValueTypeTag,
};

const NONCE: &str = "remote-v2-nonce-0123456789";
const NOW_MS: u64 = 1_900_000_000_000;

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        binding_id(id),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding ID")
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("type ID")
}

fn semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        b"query-remote-v2-tests",
    )
    .expect("managed semantics")
}

fn limits() -> RemoteLimitsV2 {
    RemoteLimitsV2 {
        deadline_ms: Some(5_000),
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 100,
        max_graph_nodes: 100,
        max_attribute_values: 100,
        max_role_players: 100,
    }
}

fn decode_limits() -> RemoteReplyDecodeLimitsV2 {
    RemoteReplyDecodeLimitsV2 {
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 100,
        max_graph_nodes: 100,
        max_attribute_values: 100,
        max_role_players: 100,
    }
}

#[derive(Clone, Copy)]
struct TestSigner;

impl RemoteReplySigner for TestSigner {
    fn public_key(&self) -> RemoteSigningPublicKey {
        RemoteSigningPublicKey::from_bytes([11; 32])
    }

    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature {
        let mut signature = [0_u8; 64];
        signature[..32].copy_from_slice(digest.as_bytes());
        signature[32..].copy_from_slice(digest.as_bytes());
        RemoteReplySignature::from_bytes(signature)
    }
}

impl RemoteReplyVerifier for TestSigner {
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool {
        key == self.public_key() && *signature == self.sign(digest)
    }
}

fn low_level_plan() -> QueryPlan {
    QueryPlan::new_v2(
        vec![binding(0, "person")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: type_id(TypeKind::Entity, "person"),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        semantics(),
    )
    .expect("low-level V2 plan")
}

fn capabilities(plan: &QueryPlan, model: bool) -> CapabilitySet {
    let mut capabilities = plan.required_capabilities().clone();
    for capability in query_remote_v2_required_capabilities(model) {
        capabilities.insert(capability);
    }
    capabilities
}

fn advertisement(plan: &QueryPlan, model: bool) -> RemoteCapabilities {
    RemoteCapabilities::new(
        capabilities(plan, model),
        RemoteExecutorBinding::new("remote-v2-test-executor", "remote-v2-test-epoch")
            .expect("executor"),
        TestSigner.public_key(),
    )
}

fn advertisement_without_hydration(plan: &QueryPlan) -> RemoteCapabilities {
    let capabilities = capabilities(plan, true)
        .iter()
        .filter(|capability| {
            !matches!(
                capability.as_str(),
                CAP_QUERY_OUTPUT_HYDRATED | CAP_QUERY_SAME_SNAPSHOT_HYDRATION
            )
        })
        .cloned()
        .collect();
    RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new("remote-v2-test-executor", "remote-v2-test-epoch")
            .expect("executor"),
        TestSigner.public_key(),
    )
}

fn request(plan: &QueryPlan, result: RemoteResultKindV2, model: bool) -> RemoteQueryRequestV2 {
    request_with_limits(plan, result, model, limits())
}

fn request_with_limits(
    plan: &QueryPlan,
    result: RemoteResultKindV2,
    model: bool,
    limits: RemoteLimitsV2,
) -> RemoteQueryRequestV2 {
    let invocation = type_bridge_contract::query_plan::QueryInvocation::new(
        plan,
        result_operation(result),
        vec![],
    )
    .expect("invocation");
    RemoteQueryRequestV2::new(
        plan,
        &invocation,
        result,
        &advertisement(plan, model),
        limits,
        NONCE,
        NOW_MS,
    )
    .expect("V2 request")
}

fn result_operation(result: RemoteResultKindV2) -> QueryOperation {
    match result {
        RemoteResultKindV2::Rows
        | RemoteResultKindV2::Documents
        | RemoteResultKindV2::HydratedRows
        | RemoteResultKindV2::HydratedPage => QueryOperation::Rows,
        RemoteResultKindV2::Count | RemoteResultKindV2::DistinctCount => QueryOperation::Count,
        RemoteResultKindV2::Exists | RemoteResultKindV2::DistinctExists => QueryOperation::Exists,
    }
}

fn person() -> TypeId {
    type_id(TypeKind::Entity, "person")
}

fn employment() -> TypeId {
    type_id(TypeKind::Relation, "employment")
}

fn employee() -> TypeId {
    type_id(TypeKind::Entity, "employee")
}

fn team() -> TypeId {
    type_id(TypeKind::Relation, "team")
}

fn model_hydration() -> HydrationProjectionV2 {
    model_hydration_with_role_flags(false, false)
}

fn model_hydration_with_role_flags(
    role_ordered: bool,
    role_distinct: bool,
) -> HydrationProjectionV2 {
    let person = person();
    let employment = employment();
    HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), person.clone(), vec![person.clone()]),
            HydrationBindingV2::new(binding_id(1), employment.clone(), vec![employment.clone()]),
        ],
        vec![
            HydrationDescriptorV2::new(
                person.clone(),
                vec![HydrationFieldV2::new(
                    "name",
                    vec![person.clone()],
                    AttributeId::new("name").expect("attribute"),
                    ValueTypeTag::String,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    true,
                )],
                vec![],
            ),
            HydrationDescriptorV2::new(
                employment.clone(),
                vec![
                    HydrationFieldV2::new(
                        "assignment_id",
                        vec![employment.clone()],
                        AttributeId::new("assignment-id").expect("attribute"),
                        ValueTypeTag::String,
                        Cardinality::new(1, Some(1)).expect("cardinality"),
                        false,
                        false,
                        true,
                    ),
                    HydrationFieldV2::new(
                        "start_date",
                        vec![employment.clone()],
                        AttributeId::new("start-date").expect("attribute"),
                        ValueTypeTag::Date,
                        Cardinality::new(0, Some(1)).expect("cardinality"),
                        false,
                        false,
                        false,
                    ),
                ],
                vec![HydrationRoleV2::new(
                    RoleId::new("employment", "employee").expect("role"),
                    vec![RoleId::new("employment", "employee").expect("role")],
                    vec![HydrationPlayerV2::new(person.clone(), vec![person])],
                    Cardinality::new(1, None).expect("cardinality"),
                    role_ordered,
                    role_distinct,
                )],
            ),
        ],
    )
}

fn model_hydration_with_mixed_declared_players() -> HydrationProjectionV2 {
    let animal = type_id(TypeKind::Entity, "animal");
    let person = person();
    let employment = employment();
    let role = RoleId::new("employment", "employee").expect("role");
    HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), person.clone(), vec![person.clone()]),
            HydrationBindingV2::new(binding_id(1), employment.clone(), vec![employment.clone()]),
        ],
        vec![
            HydrationDescriptorV2::new(animal.clone(), vec![], vec![]),
            HydrationDescriptorV2::new(
                person.clone(),
                vec![HydrationFieldV2::new(
                    "name",
                    vec![person.clone()],
                    AttributeId::new("name").expect("attribute"),
                    ValueTypeTag::String,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    true,
                )],
                vec![],
            ),
            HydrationDescriptorV2::new(
                employment.clone(),
                vec![
                    HydrationFieldV2::new(
                        "assignment_id",
                        vec![employment.clone()],
                        AttributeId::new("assignment-id").expect("attribute"),
                        ValueTypeTag::String,
                        Cardinality::new(1, Some(1)).expect("cardinality"),
                        false,
                        false,
                        true,
                    ),
                    HydrationFieldV2::new(
                        "start_date",
                        vec![employment.clone()],
                        AttributeId::new("start-date").expect("attribute"),
                        ValueTypeTag::Date,
                        Cardinality::new(0, Some(1)).expect("cardinality"),
                        false,
                        false,
                        false,
                    ),
                ],
                vec![HydrationRoleV2::new(
                    role.clone(),
                    vec![role],
                    vec![
                        HydrationPlayerV2::new(animal.clone(), vec![animal]),
                        HydrationPlayerV2::new(person.clone(), vec![person]),
                    ],
                    Cardinality::new(1, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
        ],
    )
}

fn model_output() -> QueryModelOutputV2 {
    QueryModelOutputV2::Positional {
        slots: vec![
            QueryModelOutputSlotV2::One {
                binding: binding_id(0),
                declared: person(),
            },
            QueryModelOutputSlotV2::Collect {
                binding: binding_id(1),
                declared: employment(),
                distinct: true,
                order: QueryStableOrderV2::new(
                    vec![QueryOrderTermV2::new(
                        QueryFieldV2::new(
                            binding_id(1),
                            employment(),
                            AttributeId::new("assignment-id").expect("attribute"),
                            ValueTypeTag::String,
                        ),
                        QueryOrderDirectionV2::Ascending,
                        QueryMissingOrderV2::Reject,
                    )],
                    vec![binding_id(1)],
                ),
            },
        ],
    }
}

fn page_plan() -> QueryPlan {
    page_plan_with_hydration(model_hydration())
}

fn page_plan_with_hydration(hydration: HydrationProjectionV2) -> QueryPlan {
    let person = person();
    let employment = employment();
    let order = QueryStableOrderV2::new(
        vec![QueryOrderTermV2::new(
            QueryFieldV2::new(
                binding_id(0),
                person.clone(),
                AttributeId::new("name").expect("attribute"),
                ValueTypeTag::String,
            ),
            QueryOrderDirectionV2::Ascending,
            QueryMissingOrderV2::Reject,
        )],
        vec![binding_id(0)],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person"), binding(1, "employment")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: person,
                },
                QueryPattern::Isa {
                    binding: binding_id(1),
                    include_subtypes: true,
                    type_id: employment,
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![],
            Some(ModelQueryV2::Page {
                hydration,
                include_total: true,
                order,
                output: model_output(),
                root: binding_id(0),
                window: QueryWindowV2::new(10, 25),
            }),
        ),
        semantics(),
    )
    .expect("page plan")
}

fn exactly_one_plan() -> QueryPlan {
    let person = person();
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            person.clone(),
            vec![person.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            person.clone(),
            vec![HydrationFieldV2::new(
                "name",
                vec![person.clone()],
                AttributeId::new("name").expect("attribute"),
                ValueTypeTag::String,
                Cardinality::new(1, Some(1)).expect("cardinality"),
                false,
                false,
                false,
            )],
            vec![],
        )],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: person,
                    }],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        semantics(),
    )
    .expect("exactly-one plan")
}

fn distinct_scalar_plan(exists: bool) -> QueryPlan {
    let person = person();
    let employment = employment();
    let model = if exists {
        ModelQueryV2::DistinctExists {
            hydration: model_hydration(),
            root: binding_id(0),
        }
    } else {
        ModelQueryV2::DistinctCount {
            hydration: model_hydration(),
            root: binding_id(0),
        }
    };
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person"), binding(1, "employment")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: person,
                },
                QueryPattern::Isa {
                    binding: binding_id(1),
                    include_subtypes: true,
                    type_id: employment,
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        QueryPlanV2Compatibility::new(None, vec![], Some(model)),
        semantics(),
    )
    .expect("distinct scalar plan")
}

fn inherited_rows_plan(order_owner: TypeId) -> Result<QueryPlan, Diagnostic> {
    let person = person();
    let employee = employee();
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            person.clone(),
            vec![employee.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            employee,
            vec![HydrationFieldV2::new(
                "name",
                vec![person.clone()],
                AttributeId::new("name").expect("attribute"),
                ValueTypeTag::String,
                Cardinality::new(1, Some(1)).expect("cardinality"),
                false,
                false,
                true,
            )],
            vec![],
        )],
    );
    let order = QueryStableOrderV2::new(
        vec![QueryOrderTermV2::new(
            QueryFieldV2::new(
                binding_id(0),
                order_owner,
                AttributeId::new("name").expect("attribute"),
                ValueTypeTag::String,
            ),
            QueryOrderDirectionV2::Ascending,
            QueryMissingOrderV2::Reject,
        )],
        vec![binding_id(0)],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::BoundedMany,
                hydration,
                order: Some(order),
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: person,
                    }],
                },
                window: QueryWindowV2::new(0, 10),
            }),
        ),
        semantics(),
    )
}

fn optional_order_rows_plan() -> QueryPlan {
    let person = person();
    let nickname = AttributeId::new("nickname").expect("attribute");
    let person_id = AttributeId::new("person-id").expect("attribute");
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            person.clone(),
            vec![person.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            person.clone(),
            vec![
                HydrationFieldV2::new(
                    "nickname",
                    vec![person.clone()],
                    nickname.clone(),
                    ValueTypeTag::String,
                    Cardinality::new(0, Some(1)).expect("cardinality"),
                    false,
                    false,
                    false,
                ),
                HydrationFieldV2::new(
                    "person_id",
                    vec![person.clone()],
                    person_id.clone(),
                    ValueTypeTag::String,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    true,
                ),
            ],
            vec![],
        )],
    );
    let order = QueryStableOrderV2::new(
        vec![
            QueryOrderTermV2::new(
                QueryFieldV2::new(
                    binding_id(0),
                    person.clone(),
                    nickname,
                    ValueTypeTag::String,
                ),
                QueryOrderDirectionV2::Ascending,
                QueryMissingOrderV2::Reject,
            ),
            QueryOrderTermV2::new(
                QueryFieldV2::new(
                    binding_id(0),
                    person.clone(),
                    person_id,
                    ValueTypeTag::String,
                ),
                QueryOrderDirectionV2::Ascending,
                QueryMissingOrderV2::Reject,
            ),
        ],
        vec![binding_id(0)],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::BoundedMany,
                hydration,
                order: Some(order),
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: person,
                    }],
                },
                window: QueryWindowV2::new(0, 10),
            }),
        ),
        semantics(),
    )
    .expect("optional-order rows plan")
}

fn duration_order_rows_plan() -> QueryPlan {
    let person = person();
    let elapsed = AttributeId::new("elapsed").expect("attribute");
    let person_id = AttributeId::new("person-id").expect("attribute");
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            person.clone(),
            vec![person.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            person.clone(),
            vec![
                HydrationFieldV2::new(
                    "elapsed",
                    vec![person.clone()],
                    elapsed.clone(),
                    ValueTypeTag::Duration,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    false,
                ),
                HydrationFieldV2::new(
                    "person_id",
                    vec![person.clone()],
                    person_id.clone(),
                    ValueTypeTag::String,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    true,
                ),
            ],
            vec![],
        )],
    );
    let order = QueryStableOrderV2::new(
        vec![
            QueryOrderTermV2::new(
                QueryFieldV2::new(
                    binding_id(0),
                    person.clone(),
                    elapsed,
                    ValueTypeTag::Duration,
                ),
                QueryOrderDirectionV2::Ascending,
                QueryMissingOrderV2::Reject,
            ),
            QueryOrderTermV2::new(
                QueryFieldV2::new(
                    binding_id(0),
                    person.clone(),
                    person_id,
                    ValueTypeTag::String,
                ),
                QueryOrderDirectionV2::Ascending,
                QueryMissingOrderV2::Reject,
            ),
        ],
        vec![binding_id(0)],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: true,
                type_id: person.clone(),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::BoundedMany,
                hydration,
                order: Some(order),
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: person,
                    }],
                },
                window: QueryWindowV2::new(0, 10),
            }),
        ),
        semantics(),
    )
    .expect("duration-order rows plan")
}

fn decimal_hydration_rows_plan(
    ordered: bool,
    distinct: bool,
    unique: bool,
    maximum: Option<u64>,
) -> QueryPlan {
    let person = person();
    let balance = AttributeId::new("balance").expect("attribute");
    let person_id = AttributeId::new("person-id").expect("attribute");
    let hydration = HydrationProjectionV2::new(
        vec![HydrationBindingV2::new(
            binding_id(0),
            person.clone(),
            vec![person.clone()],
        )],
        vec![HydrationDescriptorV2::new(
            person.clone(),
            vec![
                HydrationFieldV2::new(
                    "balance",
                    vec![person.clone()],
                    balance,
                    ValueTypeTag::Decimal,
                    Cardinality::new(0, maximum).expect("cardinality"),
                    ordered,
                    distinct,
                    unique,
                ),
                HydrationFieldV2::new(
                    "person_id",
                    vec![person.clone()],
                    person_id.clone(),
                    ValueTypeTag::String,
                    Cardinality::new(1, Some(1)).expect("cardinality"),
                    false,
                    false,
                    true,
                ),
            ],
            vec![],
        )],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "person")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![QueryPattern::Isa {
                binding: binding_id(0),
                include_subtypes: false,
                type_id: person.clone(),
            }],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::BoundedMany,
                hydration,
                order: Some(QueryStableOrderV2::new(
                    vec![QueryOrderTermV2::new(
                        QueryFieldV2::new(
                            binding_id(0),
                            person.clone(),
                            person_id,
                            ValueTypeTag::String,
                        ),
                        QueryOrderDirectionV2::Ascending,
                        QueryMissingOrderV2::Reject,
                    )],
                    vec![binding_id(0)],
                )),
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: person,
                    }],
                },
                window: QueryWindowV2::new(0, 10),
            }),
        ),
        semantics(),
    )
    .expect("decimal hydration rows plan")
}

fn shallow_relation_player_plan() -> QueryPlan {
    let employment = employment();
    let team = team();
    let employment_assignment = RoleId::new("employment", "assignment").expect("employment role");
    let team_parent = RoleId::new("team", "parent").expect("team role");
    let hydration = HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), employment.clone(), vec![employment.clone()]),
            HydrationBindingV2::new(binding_id(1), team.clone(), vec![team.clone()]),
        ],
        vec![
            HydrationDescriptorV2::new(
                employment.clone(),
                vec![],
                vec![HydrationRoleV2::new(
                    employment_assignment.clone(),
                    vec![employment_assignment],
                    vec![HydrationPlayerV2::new(team.clone(), vec![team.clone()])],
                    Cardinality::new(1, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
            HydrationDescriptorV2::new(
                team.clone(),
                vec![],
                vec![HydrationRoleV2::new(
                    team_parent.clone(),
                    vec![team_parent],
                    vec![HydrationPlayerV2::new(
                        employment.clone(),
                        vec![employment.clone()],
                    )],
                    Cardinality::new(1, None).expect("cardinality"),
                    false,
                    false,
                )],
            ),
        ],
    );
    QueryPlan::new_v2_with_functions(
        vec![binding(0, "employment"), binding(1, "team")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: false,
                    type_id: employment.clone(),
                },
                QueryPattern::Isa {
                    binding: binding_id(1),
                    include_subtypes: false,
                    type_id: team,
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![QueryBindingPairV2::new(binding_id(0), binding_id(1))],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![QueryModelOutputSlotV2::One {
                        binding: binding_id(0),
                        declared: employment,
                    }],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        semantics(),
    )
    .expect("shallow relation-player plan")
}

fn person_node(id: u32, iid: &str) -> HydrationNodeV2 {
    person_node_named(id, iid, "Alice")
}

fn employee_node(id: u32, iid: &str, name: &str) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        employee(),
        HydrationNodeKindV2::Entity,
        vec![HydrationAttributeEvidenceV2::new(
            AttributeId::new("name").expect("attribute"),
            vec![string_value(name)],
        )],
        vec![],
    )
}

fn person_node_named(id: u32, iid: &str, name: &str) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        person(),
        HydrationNodeKindV2::Entity,
        vec![HydrationAttributeEvidenceV2::new(
            AttributeId::new("name").expect("attribute"),
            vec![string_value(name)],
        )],
        vec![],
    )
}

fn optional_order_person_node(
    id: u32,
    iid: &str,
    nickname: Option<&str>,
    person_id: &str,
) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        person(),
        HydrationNodeKindV2::Entity,
        vec![
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("nickname").expect("attribute"),
                nickname.into_iter().map(string_value).collect(),
            ),
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("person-id").expect("attribute"),
                vec![string_value(person_id)],
            ),
        ],
        vec![],
    )
}

fn duration_order_person_node(
    id: u32,
    iid: &str,
    elapsed: CompatibilityValueV2,
    person_id: &str,
) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        person(),
        HydrationNodeKindV2::Entity,
        vec![
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("elapsed").expect("attribute"),
                vec![elapsed],
            ),
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("person-id").expect("attribute"),
                vec![string_value(person_id)],
            ),
        ],
        vec![],
    )
}

fn decimal_person_node(
    id: u32,
    iid: &str,
    balances: Vec<CompatibilityValueV2>,
    person_id: &str,
) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        person(),
        HydrationNodeKindV2::Entity,
        vec![
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("balance").expect("attribute"),
                balances,
            ),
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("person-id").expect("attribute"),
                vec![string_value(person_id)],
            ),
        ],
        vec![],
    )
}

fn empty_entity_node(id: u32, iid: &str, concrete: TypeId) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        concrete,
        HydrationNodeKindV2::Entity,
        vec![],
        vec![],
    )
}

fn employment_node(
    id: u32,
    iid: &str,
    assignment_id: &str,
    players: Vec<HydrationReferenceV2>,
) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        employment(),
        HydrationNodeKindV2::Relation,
        vec![
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("assignment-id").expect("attribute"),
                vec![string_value(assignment_id)],
            ),
            HydrationAttributeEvidenceV2::new(
                AttributeId::new("start-date").expect("attribute"),
                vec![],
            ),
        ],
        vec![HydrationRoleEvidenceV2::new(
            RoleId::new("employment", "employee").expect("role"),
            players,
        )],
    )
}

fn string_value(value: &str) -> CompatibilityValueV2 {
    CompatibilityValueV2::canonical(CanonicalValue::String(
        CanonicalString::new(value).expect("string"),
    ))
}

fn decimal_value(value: &str) -> CompatibilityValueV2 {
    CompatibilityValueV2::canonical(CanonicalValue::Decimal(
        DecimalValue::new(value).expect("decimal"),
    ))
}

fn page_graph() -> HydrationGraphV2 {
    HydrationGraphV2::new(vec![
        person_node(0, "0x01"),
        employment_node(
            1,
            "0x02",
            "assignment-1",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(0),
            )],
        ),
    ])
    .expect("page graph")
}

fn page_row() -> HydratedRowV2 {
    page_row_for(0, &[1])
}

fn page_row_for(person_id: u32, employment_ids: &[u32]) -> HydratedRowV2 {
    HydratedRowV2::new(vec![
        HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(person_id)),
        },
        HydrationSlotV2::Collection {
            values: employment_ids
                .iter()
                .map(|id| HydrationReferenceV2::new(employment(), HydrationNodeIdV2::new(*id)))
                .collect(),
        },
    ])
}

#[test]
fn v2_request_round_trips_with_distinct_fingerprint_domain_and_capability_gate() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let bytes = request.encode().expect("request bytes");
    assert_eq!(
        RemoteQueryRequestV2::decode(&bytes).expect("decode"),
        request
    );
    assert_eq!(
        request
            .fingerprint()
            .expect("fingerprint")
            .as_fingerprint()
            .canonicalization()
            .as_str(),
        QUERY_REMOTE_REQUEST_CANONICALIZATION_V2,
    );
    request
        .validate_advertisement(&advertisement(&plan, false))
        .expect("advertisement gate");

    let required = query_remote_v2_required_capabilities(true)
        .iter()
        .map(|capability| capability.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        required,
        vec![
            CAP_QUERY_SAME_SNAPSHOT_HYDRATION.to_owned(),
            CAP_QUERY_OUTPUT_HYDRATED.to_owned(),
            CAP_QUERY_PLAN_V2.to_owned(),
            CAP_QUERY_REMOTE_ENVELOPE_V2.to_owned(),
            CAP_QUERY_REMOTE_STRUCTURED_DIAGNOSTIC.to_owned(),
        ],
    );
}

#[test]
fn advertised_capability_matrix_only_requires_hydration_for_graph_results() {
    for (plan, result) in [
        (
            distinct_scalar_plan(false),
            RemoteResultKindV2::DistinctCount,
        ),
        (
            distinct_scalar_plan(true),
            RemoteResultKindV2::DistinctExists,
        ),
    ] {
        let lean = advertisement(&plan, false);
        let advertised = lean
            .capabilities()
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>();
        assert!(!advertised.contains(&CAP_QUERY_OUTPUT_HYDRATED));
        assert!(!advertised.contains(&CAP_QUERY_SAME_SNAPSHOT_HYDRATION));
        request(&plan, result, false);
    }

    for (plan, result) in [
        (exactly_one_plan(), RemoteResultKindV2::HydratedRows),
        (page_plan(), RemoteResultKindV2::HydratedPage),
    ] {
        let invocation = type_bridge_contract::query_plan::QueryInvocation::new(
            &plan,
            QueryOperation::Rows,
            vec![],
        )
        .expect("invocation");
        let error = RemoteQueryRequestV2::new(
            &plan,
            &invocation,
            result,
            &advertisement_without_hydration(&plan),
            limits(),
            NONCE,
            NOW_MS,
        )
        .expect_err("graph result needs hydration capabilities");
        assert_eq!(error.code().as_str(), "unsupported_required_capability");
        request(&plan, result, true);
    }
}

#[test]
fn request_decoder_dispatches_version_before_shape_and_rejects_unknown_fields() {
    let cross_version = br#"{"format":"typebridge.query-remote-request/v1"}"#;
    assert_eq!(
        RemoteQueryRequestV2::decode(cross_version)
            .expect_err("V1 is not V2")
            .code()
            .as_str(),
        "query_remote_v2_format_unsupported",
    );

    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let mut value: serde_json::Value =
        serde_json::from_slice(&request.encode().expect("bytes")).expect("JSON");
    value
        .as_object_mut()
        .expect("object")
        .insert("unknown".to_owned(), serde_json::json!(true));
    let bytes =
        type_bridge_contract::codec::to_canonical_json(&value).expect("canonical hostile bytes");
    assert!(RemoteQueryRequestV2::decode(&bytes).is_err());
}

#[test]
fn complete_diagnostic_survives_authenticated_v2_failure_exactly() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("example_failure").expect("code"),
        "complete structured failure",
    )
    .with_path(DiagnosticPath::from_segments([
        DiagnosticPathSegment::Field("query".to_owned()),
        DiagnosticPathSegment::Index(7),
        DiagnosticPathSegment::Identifier("person:name".to_owned()),
    ]))
    .with_detail("signed_long", -9_i64)
    .with_detail("retryable", false)
    .with_detail("expected", vec!["person".to_owned(), "employee".to_owned()])
    .with_detail("hint", "register the concrete subtype");
    let failure =
        RemoteQueryFailureV2::bound(NONCE, &request_fingerprint, &diagnostic).expect("failure");
    let advertisement = advertisement(&plan, false);
    let bytes = failure
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed failure");
    let reply = decode_remote_reply_v2(
        &bytes,
        &request,
        &plan.fingerprint().expect("plan fingerprint"),
        &request_fingerprint,
        &advertisement.fingerprint().expect("advertisement"),
        advertisement.reply_key(),
        decode_limits(),
        &TestSigner,
    )
    .expect("decode");
    let RemoteReplyV2::Failure(decoded) = reply else {
        panic!("expected failure");
    };
    assert_eq!(decoded.diagnostic().expect("diagnostic"), diagnostic);
}

#[test]
fn reply_decoder_rejects_incoherent_trusted_request_bindings() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::Rows,
        RemoteOutcomeV2::Rows { rows: vec![] },
    )
    .expect("response");
    let advertisement = advertisement(&plan, false);
    let bytes = response
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed response");

    let mut other_limits = limits();
    other_limits.max_items -= 1;
    let other_request = request_with_limits(&plan, RemoteResultKindV2::Rows, false, other_limits);
    assert_eq!(
        decode_remote_reply_v2(
            &bytes,
            &request,
            &plan.fingerprint().expect("plan fingerprint"),
            &other_request
                .fingerprint()
                .expect("other request fingerprint"),
            &advertisement.fingerprint().expect("advertisement"),
            advertisement.reply_key(),
            decode_limits(),
            &TestSigner,
        )
        .expect_err("incoherent trusted request fingerprint")
        .code()
        .as_str(),
        "query_remote_v2_expected_binding_mismatch",
    );
}

#[test]
fn pre_request_failure_decoder_rejects_request_bound_failures() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("example_failure").expect("code"),
        "failure",
    );
    let advertisement = advertisement(&plan, false);
    let advertisement_fingerprint = advertisement.fingerprint().expect("advertisement");
    let bound = RemoteQueryFailureV2::bound(
        NONCE,
        &request.fingerprint().expect("request fingerprint"),
        &diagnostic,
    )
    .expect("bound failure");
    let bytes = bound
        .encode_signed(&advertisement_fingerprint, &TestSigner)
        .expect("signed bound failure");
    assert_eq!(
        decode_signed_remote_failure_v2(
            &bytes,
            &advertisement_fingerprint,
            advertisement.reply_key(),
            limits().max_bytes,
            &TestSigner,
        )
        .expect_err("request-bound failure is not a pre-request failure")
        .code()
        .as_str(),
        "query_remote_v2_failure_unexpected_binding",
    );

    let unbound =
        RemoteQueryFailureV2::new(Some(NONCE.to_owned()), &diagnostic).expect("unbound failure");
    let bytes = unbound
        .encode_signed(&advertisement_fingerprint, &TestSigner)
        .expect("signed unbound failure");
    assert_eq!(
        decode_signed_remote_failure_v2(
            &bytes,
            &advertisement_fingerprint,
            advertisement.reply_key(),
            limits().max_bytes,
            &TestSigner,
        )
        .expect("pre-request failure"),
        unbound,
    );
}

#[test]
fn diagnostic_long_and_unknown_nested_fields_fail_closed() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let fingerprint = request.fingerprint().expect("fingerprint");
    let failure = RemoteQueryFailureV2::bound(
        NONCE,
        &fingerprint,
        &Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new("example_failure").expect("code"),
            "failure",
        )
        .with_detail("signed_long", -9_i64),
    )
    .expect("failure");
    let bytes = serde_json::to_vec(&failure).expect("JSON");
    let text = String::from_utf8(bytes).expect("UTF-8");
    let noncanonical = text.replace(r#""value":"-9""#, r#""value":"-09""#);
    assert_eq!(
        RemoteQueryFailureV2::decode_payload(noncanonical.as_bytes())
            .expect_err("noncanonical long")
            .code()
            .as_str(),
        "query_remote_v2_diagnostic_long_invalid",
    );
    let unknown = text.replace(r#""value":"-9""#, r#""extra":true,"value":"-9""#);
    assert!(RemoteQueryFailureV2::decode_payload(unknown.as_bytes()).is_err());
}

#[test]
fn low_level_signed_response_round_trips_and_preflights_item_budget() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let row = vec![type_bridge_contract::query_remote::RemoteValue::Thing {
        iid: "0x01".to_owned(),
        type_id: person(),
    }];
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::Rows,
        RemoteOutcomeV2::Rows {
            rows: vec![row.clone(), row],
        },
    )
    .expect("response");
    let advertisement = advertisement(&plan, false);
    let bytes = response
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed response");
    let mut tight = decode_limits();
    tight.max_items = 1;
    assert_eq!(
        decode_remote_reply_v2(
            &bytes,
            &request,
            &plan.fingerprint().expect("plan fingerprint"),
            &request_fingerprint,
            &advertisement.fingerprint().expect("advertisement"),
            advertisement.reply_key(),
            tight,
            &TestSigner,
        )
        .expect_err("budget")
        .code()
        .as_str(),
        "query_remote_v2_item_limit",
    );
}

#[test]
fn scalar_count_remains_lossless_above_collection_ceiling() {
    const COUNT: u64 = 100_000;
    let plan = low_level_plan();
    let mut request_limits = limits();
    request_limits.max_items = COUNT + 1;
    let request = request_with_limits(&plan, RemoteResultKindV2::Count, false, request_limits);
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::Count,
        RemoteOutcomeV2::Count { value: COUNT },
    )
    .expect("lossless scalar response");
    let advertisement = advertisement(&plan, false);
    let bytes = response
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed response");
    let mut accepted = decode_limits();
    accepted.max_items = COUNT + 1;
    assert!(matches!(
        decode_remote_reply_v2(
            &bytes,
            &request,
            &plan.fingerprint().expect("plan fingerprint"),
            &request_fingerprint,
            &advertisement.fingerprint().expect("advertisement"),
            advertisement.reply_key(),
            accepted,
            &TestSigner,
        )
        .expect("lossless decode"),
        RemoteReplyV2::Response(_)
    ));
    accepted.max_items = COUNT - 1;
    assert_eq!(
        decode_remote_reply_v2(
            &bytes,
            &request,
            &plan.fingerprint().expect("plan fingerprint"),
            &request_fingerprint,
            &advertisement.fingerprint().expect("advertisement"),
            advertisement.reply_key(),
            accepted,
            &TestSigner,
        )
        .expect_err("caller-tightened scalar budget")
        .code()
        .as_str(),
        "query_remote_v2_item_limit",
    );
}

#[test]
fn hydrated_evidence_round_trips_released_strings_above_canonical_string_limit() {
    let plan = exactly_one_plan();
    let mut request_limits = limits();
    request_limits.max_bytes = 4 << 20;
    let request = request_with_limits(
        &plan,
        RemoteResultKindV2::HydratedRows,
        true,
        request_limits,
    );
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let released = "x".repeat((1 << 20) + 1);
    let graph = HydrationGraphV2::new(vec![HydrationNodeV2::new(
        HydrationNodeIdV2::new(0),
        "0x01".to_owned(),
        person(),
        HydrationNodeKindV2::Entity,
        vec![HydrationAttributeEvidenceV2::new(
            AttributeId::new("name").expect("attribute"),
            vec![
                CompatibilityValueV2::released_string(released.clone())
                    .expect("released long string"),
            ],
        )],
        vec![],
    )])
    .expect("graph");
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::HydratedRows,
        RemoteOutcomeV2::HydratedRows {
            graph,
            rows: vec![HydratedRowV2::new(vec![HydrationSlotV2::Singular {
                value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
            }])],
        },
    )
    .expect("response");
    let advertisement = advertisement(&plan, true);
    let bytes = response
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed response");
    let mut reply_limits = decode_limits();
    reply_limits.max_bytes = request_limits.max_bytes;
    let RemoteReplyV2::Response(decoded) = decode_remote_reply_v2(
        &bytes,
        &request,
        &plan.fingerprint().expect("plan fingerprint"),
        &request_fingerprint,
        &advertisement.fingerprint().expect("advertisement"),
        advertisement.reply_key(),
        reply_limits,
        &TestSigner,
    )
    .expect("decode") else {
        panic!("expected response");
    };
    let RemoteOutcomeV2::HydratedRows { graph, .. } = decoded.into_outcome() else {
        panic!("expected hydrated rows");
    };
    assert_eq!(
        graph.nodes()[0].attributes()[0].values()[0].released_text(),
        Some(released),
    );
}

#[test]
fn hydrated_page_validates_graph_projection_window_total_and_budgets() {
    let plan = page_plan();
    let request = request(&plan, RemoteResultKindV2::HydratedPage, true);
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::HydratedPage,
        RemoteOutcomeV2::HydratedPage {
            entries: vec![page_row()],
            graph: page_graph(),
            limit: 25,
            offset: 10,
            root: binding_id(0),
            total: Some(11),
        },
    )
    .expect("page response");
    let advertisement = advertisement(&plan, true);
    let bytes = response
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed response");
    let framing_floor = RemoteQueryResponseV2::signed_framing_floor(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::HydratedPage,
        &advertisement.fingerprint().expect("advertisement"),
        advertisement.reply_key(),
        decode_limits().max_items,
    )
    .expect("model response framing floor");
    assert!(
        framing_floor <= bytes.len(),
        "the pre-host framing floor cannot overstate a valid signed response",
    );
    let reply = decode_remote_reply_v2(
        &bytes,
        &request,
        &plan.fingerprint().expect("plan fingerprint"),
        &request_fingerprint,
        &advertisement.fingerprint().expect("advertisement"),
        advertisement.reply_key(),
        decode_limits(),
        &TestSigner,
    )
    .expect("page decode");
    assert!(matches!(reply, RemoteReplyV2::Response(_)));

    let mut tight = decode_limits();
    tight.max_graph_nodes = 1;
    assert_eq!(
        decode_remote_reply_v2(
            &bytes,
            &request,
            &plan.fingerprint().expect("plan fingerprint"),
            &request_fingerprint,
            &advertisement.fingerprint().expect("advertisement"),
            advertisement.reply_key(),
            tight,
            &TestSigner,
        )
        .expect_err("node budget")
        .code()
        .as_str(),
        "query_remote_v2_graph_node_limit",
    );
}

#[test]
fn local_compatibility_outcomes_use_the_same_plan_and_budget_gate_as_wire_decode() {
    let plan = page_plan();
    let outcome = RemoteOutcomeV2::HydratedPage {
        entries: vec![page_row()],
        graph: page_graph(),
        limit: 25,
        offset: 10,
        root: binding_id(0),
        total: Some(11),
    };
    validate_remote_outcome_v2(
        &outcome,
        RemoteResultKindV2::HydratedPage,
        decode_limits(),
        &plan,
    )
    .expect("locally constructed compatibility evidence");

    let mut tight = decode_limits();
    tight.max_graph_nodes = 1;
    assert_eq!(
        validate_remote_outcome_v2(&outcome, RemoteResultKindV2::HydratedPage, tight, &plan,)
            .expect_err("local evidence cannot bypass graph budgets")
            .code()
            .as_str(),
        "query_remote_v2_graph_node_limit",
    );
    assert_eq!(
        validate_remote_outcome_v2(
            &outcome,
            RemoteResultKindV2::HydratedRows,
            decode_limits(),
            &plan,
        )
        .expect_err("local evidence cannot change the terminal family")
        .code()
        .as_str(),
        "query_remote_v2_outcome_mismatch",
    );
}

#[test]
fn request_bound_v2_limits_cannot_be_widened_and_each_graph_budget_preflights() {
    let plan = page_plan();
    let request = request(&plan, RemoteResultKindV2::HydratedPage, true);
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::HydratedPage,
        RemoteOutcomeV2::HydratedPage {
            entries: vec![page_row()],
            graph: page_graph(),
            limit: 25,
            offset: 10,
            root: binding_id(0),
            total: Some(11),
        },
    )
    .expect("page response");
    let advertisement = advertisement(&plan, true);
    let bytes = response
        .encode_signed(
            &advertisement.fingerprint().expect("advertisement"),
            &TestSigner,
        )
        .expect("signed response");
    let decode = |limits| {
        decode_remote_reply_v2(
            &bytes,
            &request,
            &plan.fingerprint().expect("plan fingerprint"),
            &request_fingerprint,
            &advertisement.fingerprint().expect("advertisement"),
            advertisement.reply_key(),
            limits,
            &TestSigner,
        )
    };

    let mut widened = decode_limits();
    widened.max_graph_nodes += 1;
    assert_eq!(
        decode(widened).expect_err("widened budget").code().as_str(),
        "query_remote_v2_limits_widened",
    );

    for (mut limits, expected) in [
        {
            let mut limits = decode_limits();
            limits.max_collection_members = 0;
            (limits, "query_remote_v2_collection_member_limit")
        },
        {
            let mut limits = decode_limits();
            limits.max_attribute_values = 1;
            (limits, "query_remote_v2_attribute_value_limit")
        },
        {
            let mut limits = decode_limits();
            limits.max_role_players = 0;
            (limits, "query_remote_v2_role_player_limit")
        },
    ] {
        // Keep the binding explicit so a future test-table edit cannot
        // accidentally reuse a widened mutable limit.
        limits.max_bytes = limits.max_bytes.min(request.limits().max_bytes);
        assert_eq!(
            decode(limits).expect_err("tight budget").code().as_str(),
            expected,
        );
    }
}

#[test]
fn hydrated_page_and_collection_order_are_proved_from_graph_values() {
    let plan = page_plan();
    let page_request = request(&plan, RemoteResultKindV2::HydratedPage, true);
    let fingerprint = page_request.fingerprint().expect("fingerprint");
    let graph = HydrationGraphV2::new(vec![
        person_node_named(0, "0x01", "Alice"),
        employment_node(
            1,
            "0x02",
            "assignment-1",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(0),
            )],
        ),
        person_node_named(2, "0x03", "Bob"),
        employment_node(
            3,
            "0x04",
            "assignment-2",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(2),
            )],
        ),
    ])
    .expect("graph");
    let ordered = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &fingerprint,
        RemoteResultKindV2::HydratedPage,
        RemoteOutcomeV2::HydratedPage {
            entries: vec![page_row_for(0, &[1]), page_row_for(2, &[3])],
            graph: graph.clone(),
            limit: 25,
            offset: 10,
            root: binding_id(0),
            total: Some(12),
        },
    );
    assert!(ordered.is_ok());
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row_for(2, &[3]), page_row_for(0, &[1])],
                graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(12),
            },
        )
        .is_err(),
        "reversed page rows must fail stable-order validation",
    );

    let collection_graph = HydrationGraphV2::new(vec![
        person_node_named(0, "0x01", "Alice"),
        employment_node(
            1,
            "0x02",
            "assignment-1",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(0),
            )],
        ),
        employment_node(
            2,
            "0x03",
            "assignment-2",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(0),
            )],
        ),
    ])
    .expect("collection graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row_for(0, &[2, 1])],
                graph: collection_graph.clone(),
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err(),
        "reversed collection members must fail their independent order",
    );
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row_for(0, &[1, 1])],
                graph: collection_graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err(),
        "distinct collection output must reject duplicate identities",
    );
}

#[test]
fn reject_missing_order_matches_v1_pairwise_semantics() {
    let plan = optional_order_rows_plan();
    let request = request(&plan, RemoteResultKindV2::HydratedRows, true);
    let fingerprint = request.fingerprint().expect("fingerprint");
    let row = |node| {
        HydratedRowV2::new(vec![HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(node)),
        }])
    };

    let both_missing = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &fingerprint,
        RemoteResultKindV2::HydratedRows,
        RemoteOutcomeV2::HydratedRows {
            graph: HydrationGraphV2::new(vec![
                optional_order_person_node(0, "0x01", None, "person-1"),
                optional_order_person_node(1, "0x02", None, "person-2"),
            ])
            .expect("graph"),
            rows: vec![row(0), row(1)],
        },
    );
    assert!(
        both_missing.is_ok(),
        "two missing values compare equal before the required unique tie term",
    );

    let singleton_missing = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &fingerprint,
        RemoteResultKindV2::HydratedRows,
        RemoteOutcomeV2::HydratedRows {
            graph: HydrationGraphV2::new(vec![optional_order_person_node(
                0, "0x01", None, "person-1",
            )])
            .expect("graph"),
            rows: vec![row(0)],
        },
    );
    assert!(
        singleton_missing.is_ok(),
        "a singleton has no adjacent order comparison",
    );

    let missing_against_present = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &fingerprint,
        RemoteResultKindV2::HydratedRows,
        RemoteOutcomeV2::HydratedRows {
            graph: HydrationGraphV2::new(vec![
                optional_order_person_node(0, "0x01", None, "person-1"),
                optional_order_person_node(1, "0x02", Some("Bob"), "person-2"),
            ])
            .expect("graph"),
            rows: vec![row(0), row(1)],
        },
    );
    assert_eq!(
        missing_against_present
            .expect_err("Reject must fail a missing-versus-present comparison")
            .code()
            .as_str(),
        "query_remote_v2_evidence_mismatch",
    );
}

#[test]
fn duration_order_uses_released_v1_component_semantics() {
    let plan = duration_order_rows_plan();
    let request = request(&plan, RemoteResultKindV2::HydratedRows, true);
    let fingerprint = request.fingerprint().expect("fingerprint");
    let day = CompatibilityValueV2::released_duration("PT24H").expect("released duration");
    let days = CompatibilityValueV2::canonical(CanonicalValue::Duration(
        "P30D".parse().expect("canonical duration"),
    ));
    let month = CompatibilityValueV2::canonical(CanonicalValue::Duration(
        "P1M".parse().expect("canonical duration"),
    ));
    let graph = HydrationGraphV2::new(vec![
        duration_order_person_node(0, "0x01", day, "person-1"),
        duration_order_person_node(1, "0x02", days, "person-2"),
        duration_order_person_node(2, "0x03", month, "person-3"),
    ])
    .expect("duration graph");
    let row = |node| {
        HydratedRowV2::new(vec![HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(node)),
        }])
    };
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows {
                graph: graph.clone(),
                rows: vec![row(0), row(1), row(2)],
            },
        )
        .is_ok(),
        "released V1 compares PT24H < P30D < P1M by (months, days, nanos)",
    );
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows {
                graph,
                rows: vec![row(2), row(1), row(0)],
            },
        )
        .is_err(),
        "lexical or reversed duration order must not pass as released V1 order",
    );
}

#[test]
fn released_decimal_aliases_cannot_forge_distinct_or_unique_hydration_evidence() {
    let canonical = || decimal_value("1");
    let released = || CompatibilityValueV2::released_decimal("1.0dec").expect("released decimal");
    let row = |node| {
        HydratedRowV2::new(vec![HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(node)),
        }])
    };
    let assert_rejected =
        |plan: QueryPlan, nodes: Vec<HydrationNodeV2>, rows: Vec<HydratedRowV2>, expected: &str| {
            let request = request(&plan, RemoteResultKindV2::HydratedRows, true);
            let graph = HydrationGraphV2::new(nodes).expect("structurally canonical forged graph");
            let error = RemoteQueryResponseV2::new(
                NONCE,
                &plan,
                &request.fingerprint().expect("fingerprint"),
                RemoteResultKindV2::HydratedRows,
                RemoteOutcomeV2::HydratedRows { graph, rows },
            )
            .expect_err("semantic decimal duplicate must fail closed");
            assert_eq!(error.code().as_str(), "query_remote_v2_evidence_mismatch");
            assert_eq!(error.message(), expected);
        };

    assert_rejected(
        decimal_hydration_rows_plan(false, false, false, None),
        vec![decimal_person_node(
            0,
            "0x01",
            vec![canonical(), released()],
            "person-1",
        )],
        vec![row(0)],
        "unordered hydration attribute values must be semantically sorted and unique",
    );
    assert_rejected(
        decimal_hydration_rows_plan(true, true, false, None),
        vec![decimal_person_node(
            0,
            "0x01",
            vec![canonical(), released()],
            "person-1",
        )],
        vec![row(0)],
        "distinct ordered hydration attribute contains duplicate values",
    );
    assert_rejected(
        decimal_hydration_rows_plan(false, false, true, Some(1)),
        vec![
            decimal_person_node(0, "0x01", vec![canonical()], "person-1"),
            decimal_person_node(1, "0x02", vec![released()], "person-2"),
        ],
        vec![row(0), row(1)],
        "unique hydration field repeats one value across provider identities",
    );
}

#[test]
fn inherited_order_field_uses_reference_owner_authority_not_occurrence_equality() {
    let plan = inherited_rows_plan(person()).expect("inherited field plan");
    let request = request(&plan, RemoteResultKindV2::HydratedRows, true);
    let graph = HydrationGraphV2::new(vec![
        employee_node(0, "0x01", "Alice"),
        employee_node(1, "0x02", "Bob"),
    ])
    .expect("subtype graph");
    let rows = vec![
        HydratedRowV2::new(vec![HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
        }]),
        HydratedRowV2::new(vec![HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(1)),
        }]),
    ];
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &request.fingerprint().expect("fingerprint"),
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows { graph, rows },
        )
        .is_ok(),
        "a parent-owned inherited field must order concrete subtype occurrences",
    );
    assert!(
        inherited_rows_plan(type_id(TypeKind::Entity, "animal")).is_err(),
        "an unrelated field owner must not cross the plan authority boundary",
    );
}

#[test]
fn unrelated_unique_field_owners_may_share_one_attribute_value() {
    let person = person();
    let animal = type_id(TypeKind::Entity, "animal");
    let field = |owner: TypeId| {
        HydrationFieldV2::new(
            "name",
            vec![owner],
            AttributeId::new("name").expect("attribute"),
            ValueTypeTag::String,
            Cardinality::new(1, Some(1)).expect("cardinality"),
            false,
            false,
            true,
        )
    };
    let hydration = HydrationProjectionV2::new(
        vec![
            HydrationBindingV2::new(binding_id(0), person.clone(), vec![person.clone()]),
            HydrationBindingV2::new(binding_id(1), animal.clone(), vec![animal.clone()]),
        ],
        vec![
            HydrationDescriptorV2::new(animal.clone(), vec![field(animal.clone())], vec![]),
            HydrationDescriptorV2::new(person.clone(), vec![field(person.clone())], vec![]),
        ],
    );
    let plan = QueryPlan::new_v2_with_functions(
        vec![binding(0, "person"), binding(1, "animal")],
        vec![],
        vec![],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: false,
                    type_id: person.clone(),
                },
                QueryPattern::Isa {
                    binding: binding_id(1),
                    include_subtypes: false,
                    type_id: animal.clone(),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        QueryPlanV2Compatibility::new(
            None,
            vec![QueryBindingPairV2::new(binding_id(0), binding_id(1))],
            Some(ModelQueryV2::Rows {
                cardinality: QueryRowCardinalityV2::ExactlyOne,
                hydration,
                order: None,
                output: QueryModelOutputV2::Positional {
                    slots: vec![
                        QueryModelOutputSlotV2::One {
                            binding: binding_id(0),
                            declared: person.clone(),
                        },
                        QueryModelOutputSlotV2::One {
                            binding: binding_id(1),
                            declared: animal.clone(),
                        },
                    ],
                },
                window: QueryWindowV2::new(0, 1),
            }),
        ),
        semantics(),
    )
    .expect("unrelated-owner plan");
    let graph = HydrationGraphV2::new(vec![
        HydrationNodeV2::new(
            HydrationNodeIdV2::new(0),
            "0x01".to_owned(),
            person.clone(),
            HydrationNodeKindV2::Entity,
            vec![HydrationAttributeEvidenceV2::new(
                AttributeId::new("name").expect("attribute"),
                vec![string_value("Shared")],
            )],
            vec![],
        ),
        HydrationNodeV2::new(
            HydrationNodeIdV2::new(1),
            "0x02".to_owned(),
            animal.clone(),
            HydrationNodeKindV2::Entity,
            vec![HydrationAttributeEvidenceV2::new(
                AttributeId::new("name").expect("attribute"),
                vec![string_value("Shared")],
            )],
            vec![],
        ),
    ])
    .expect("graph");
    let rows = vec![HydratedRowV2::new(vec![
        HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(person, HydrationNodeIdV2::new(0)),
        },
        HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(animal, HydrationNodeIdV2::new(1)),
        },
    ])];
    let request = request(&plan, RemoteResultKindV2::HydratedRows, true);
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &request.fingerprint().expect("fingerprint"),
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows { graph, rows },
        )
        .is_ok(),
        "owner-scoped unique claims must not collide by attribute label alone",
    );
}

#[test]
fn role_player_authority_and_ordering_fail_closed() {
    let plan = page_plan();
    let page_request = request(&plan, RemoteResultKindV2::HydratedPage, true);
    let fingerprint = page_request.fingerprint().expect("fingerprint");
    let forged_declared = type_id(TypeKind::Entity, "animal");
    let forged_graph = HydrationGraphV2::new(vec![
        person_node(0, "0x01"),
        employment_node(
            1,
            "0x02",
            "assignment-1",
            vec![HydrationReferenceV2::new(
                forged_declared,
                HydrationNodeIdV2::new(0),
            )],
        ),
    ])
    .expect("structurally valid same-kind graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row()],
                graph: forged_graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err(),
        "same-kind role declarations still require exact declared-to-concrete authority",
    );

    let disconnected_graph = HydrationGraphV2::new(vec![
        person_node_named(0, "0x01", "Alice"),
        employment_node(
            1,
            "0x02",
            "assignment-1",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(0),
            )],
        ),
        person_node_named(2, "0x03", "Bob"),
        employment_node(
            3,
            "0x04",
            "assignment-2",
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(2),
            )],
        ),
    ])
    .expect("structurally valid disconnected graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row()],
                graph: disconnected_graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err(),
        "a detached role-owner/player component must not mark itself reachable",
    );

    let unordered_graph = HydrationGraphV2::new(vec![
        person_node_named(0, "0x01", "Alice"),
        person_node_named(1, "0x02", "Bob"),
        employment_node(
            2,
            "0x03",
            "assignment-1",
            vec![
                HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(1)),
                HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
            ],
        ),
    ])
    .expect("structurally valid graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row_for(0, &[2])],
                graph: unordered_graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err(),
        "unordered roles must use canonical provider-identity order",
    );

    let mixed_plan = page_plan_with_hydration(model_hydration_with_mixed_declared_players());
    let mixed_request = request(&mixed_plan, RemoteResultKindV2::HydratedPage, true);
    let animal = type_id(TypeKind::Entity, "animal");
    let mixed_graph = |players| {
        HydrationGraphV2::new(vec![
            person_node(0, "0x01"),
            empty_entity_node(1, "0x02", animal.clone()),
            employment_node(2, "0x03", "assignment-1", players),
        ])
        .expect("mixed-declared graph")
    };
    let mixed_outcome = |graph| RemoteOutcomeV2::HydratedPage {
        entries: vec![page_row_for(0, &[2])],
        graph,
        limit: 25,
        offset: 10,
        root: binding_id(0),
        total: Some(11),
    };
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &mixed_plan,
            &mixed_request.fingerprint().expect("fingerprint"),
            RemoteResultKindV2::HydratedPage,
            mixed_outcome(mixed_graph(vec![
                HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
                HydrationReferenceV2::new(animal.clone(), HydrationNodeIdV2::new(1)),
            ])),
        )
        .is_ok(),
        "unordered role occurrences are canonical by provider identity before declared view",
    );
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &mixed_plan,
            &mixed_request.fingerprint().expect("fingerprint"),
            RemoteResultKindV2::HydratedPage,
            mixed_outcome(mixed_graph(vec![
                HydrationReferenceV2::new(animal.clone(), HydrationNodeIdV2::new(1)),
                HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
            ])),
        )
        .is_err(),
        "declared-descriptor ordering cannot mask reversed provider identities",
    );

    let distinct_plan = page_plan_with_hydration(model_hydration_with_role_flags(true, true));
    let distinct_request = request(&distinct_plan, RemoteResultKindV2::HydratedPage, true);
    let duplicate_graph = HydrationGraphV2::new(vec![
        person_node(0, "0x01"),
        employment_node(
            1,
            "0x02",
            "assignment-1",
            vec![
                HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
                HydrationReferenceV2::new(person(), HydrationNodeIdV2::new(0)),
            ],
        ),
    ])
    .expect("structurally valid graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &distinct_plan,
            &distinct_request.fingerprint().expect("fingerprint"),
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row()],
                graph: duplicate_graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err(),
        "distinct ordered roles must reject repeated provider identities",
    );
}

#[test]
fn relation_role_players_are_shallow_and_cannot_smuggle_recursive_roles() {
    let plan = shallow_relation_player_plan();
    let request = request(&plan, RemoteResultKindV2::HydratedRows, true);
    let fingerprint = request.fingerprint().expect("fingerprint");
    let row = HydratedRowV2::new(vec![HydrationSlotV2::Singular {
        value: HydrationReferenceV2::new(employment(), HydrationNodeIdV2::new(0)),
    }]);
    let employment_node = || {
        HydrationNodeV2::new(
            HydrationNodeIdV2::new(0),
            "0x01".to_owned(),
            employment(),
            HydrationNodeKindV2::Relation,
            vec![],
            vec![HydrationRoleEvidenceV2::new(
                RoleId::new("employment", "assignment").expect("role"),
                vec![HydrationReferenceV2::new(team(), HydrationNodeIdV2::new(1))],
            )],
        )
    };
    let shallow_team = HydrationNodeV2::new(
        HydrationNodeIdV2::new(1),
        "0x02".to_owned(),
        team(),
        HydrationNodeKindV2::Relation,
        vec![],
        vec![],
    );
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows {
                graph: HydrationGraphV2::new(vec![employment_node(), shallow_team])
                    .expect("shallow graph"),
                rows: vec![row.clone()],
            },
        )
        .is_ok(),
        "released materializers admit relation players with attributes but no recursive roles",
    );

    let recursive_team = HydrationNodeV2::new(
        HydrationNodeIdV2::new(1),
        "0x02".to_owned(),
        team(),
        HydrationNodeKindV2::Relation,
        vec![],
        vec![HydrationRoleEvidenceV2::new(
            RoleId::new("team", "parent").expect("role"),
            vec![HydrationReferenceV2::new(
                employment(),
                HydrationNodeIdV2::new(0),
            )],
        )],
    );
    let recursive_graph = HydrationGraphV2::new(vec![employment_node(), recursive_team])
        .expect("structurally closed cyclic graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &plan,
            &fingerprint,
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows {
                graph: recursive_graph,
                rows: vec![row],
            },
        )
        .is_err(),
        "a shallow relation-player occurrence must not carry recursively materialized roles",
    );
}

#[test]
fn hydration_graph_rejects_dense_iid_kind_role_and_reference_contradictions() {
    assert_eq!(
        HydrationGraphV2::new(vec![person_node(1, "0x01")])
            .expect_err("not dense")
            .code()
            .as_str(),
        "query_remote_v2_evidence_mismatch",
    );
    assert!(HydrationGraphV2::new(vec![person_node(0, "0x01"), person_node(1, "0x01")]).is_err());
    let entity_with_role = HydrationNodeV2::new(
        HydrationNodeIdV2::new(0),
        "0x01".to_owned(),
        person(),
        HydrationNodeKindV2::Entity,
        vec![],
        vec![HydrationRoleEvidenceV2::new(
            RoleId::new("employment", "employee").expect("role"),
            vec![],
        )],
    );
    assert!(HydrationGraphV2::new(vec![entity_with_role]).is_err());
    let relation_with_foreign_ref = HydrationNodeV2::new(
        HydrationNodeIdV2::new(0),
        "0x02".to_owned(),
        employment(),
        HydrationNodeKindV2::Relation,
        vec![],
        vec![HydrationRoleEvidenceV2::new(
            RoleId::new("employment", "employee").expect("role"),
            vec![HydrationReferenceV2::new(
                person(),
                HydrationNodeIdV2::new(9),
            )],
        )],
    );
    assert!(HydrationGraphV2::new(vec![relation_with_foreign_ref]).is_err());
}

#[test]
fn projection_rejects_declared_type_cardinality_and_exactly_one_mismatches() {
    let page_plan = page_plan();
    let page_request = request(&page_plan, RemoteResultKindV2::HydratedPage, true);
    let page_fingerprint = page_request.fingerprint().expect("fingerprint");
    let wrong_declared = HydratedRowV2::new(vec![
        HydrationSlotV2::Singular {
            value: HydrationReferenceV2::new(employment(), HydrationNodeIdV2::new(0)),
        },
        HydrationSlotV2::Collection {
            values: vec![HydrationReferenceV2::new(
                employment(),
                HydrationNodeIdV2::new(1),
            )],
        },
    ]);
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &page_plan,
            &page_fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![wrong_declared],
                graph: page_graph(),
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err()
    );

    let incomplete_graph = HydrationGraphV2::new(vec![
        HydrationNodeV2::new(
            HydrationNodeIdV2::new(0),
            "0x01".to_owned(),
            person(),
            HydrationNodeKindV2::Entity,
            vec![],
            vec![],
        ),
        page_graph().nodes()[1].clone(),
    ])
    .expect("structurally valid graph");
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &page_plan,
            &page_fingerprint,
            RemoteResultKindV2::HydratedPage,
            RemoteOutcomeV2::HydratedPage {
                entries: vec![page_row()],
                graph: incomplete_graph,
                limit: 25,
                offset: 10,
                root: binding_id(0),
                total: Some(11),
            },
        )
        .is_err()
    );

    let one_plan = exactly_one_plan();
    let one_request = request(&one_plan, RemoteResultKindV2::HydratedRows, true);
    assert!(
        RemoteQueryResponseV2::new(
            NONCE,
            &one_plan,
            &one_request.fingerprint().expect("fingerprint"),
            RemoteResultKindV2::HydratedRows,
            RemoteOutcomeV2::HydratedRows {
                graph: HydrationGraphV2::new(vec![]).expect("empty graph"),
                rows: vec![],
            },
        )
        .is_err()
    );
}

#[test]
fn v2_wire_goldens_are_independent_from_v1() {
    let plan = low_level_plan();
    let request = request(&plan, RemoteResultKindV2::Rows, false);
    let request_bytes = request.encode().expect("request bytes");
    let request_fingerprint = request.fingerprint().expect("request fingerprint");
    let response = RemoteQueryResponseV2::new(
        NONCE,
        &plan,
        &request_fingerprint,
        RemoteResultKindV2::Rows,
        RemoteOutcomeV2::Rows { rows: vec![] },
    )
    .expect("response");
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("golden_failure").expect("code"),
        "golden structured failure",
    )
    .with_path(DiagnosticPath::from_segments([
        DiagnosticPathSegment::Index(3),
    ]))
    .with_detail("expected", 7_i64);
    let failure =
        RemoteQueryFailureV2::bound(NONCE, &request_fingerprint, &diagnostic).expect("failure");
    let advertisement = advertisement(&plan, false);
    let advertisement_fingerprint = advertisement.fingerprint().expect("advertisement");
    let response_bytes = response
        .encode_signed(&advertisement_fingerprint, &TestSigner)
        .expect("response bytes");
    let failure_bytes = failure
        .encode_signed(&advertisement_fingerprint, &TestSigner)
        .expect("failure bytes");
    assert_eq!(
        hex_digest(&request_bytes),
        "f5f32d47d6dda4a6d4153422deac09466c2ebb022f1dd57ebadbabd96fa600b8",
    );
    assert_eq!(
        request_fingerprint.as_fingerprint().digest().to_hex(),
        "49c88eddc0eb783290c8604ea8c6b95b1195eb36a34fd951232b64f222d83632",
    );
    assert_eq!(
        hex_digest(&response_bytes),
        "fee6f23d9044153adca1e18735be3a0cf25f3e2061438e4d6645f620afca9bb9",
    );
    assert_eq!(
        hex_digest(&failure_bytes),
        "203ae1cf589c53658299c4641c1d1a7288b25477ae7197c89d81f36896eefb92",
    );
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
