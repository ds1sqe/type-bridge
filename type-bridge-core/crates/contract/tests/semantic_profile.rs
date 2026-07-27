use type_bridge_contract::{
    fingerprint::SemanticProfileId,
    semantic_profile::{InterfaceKind, SemanticProfile},
    value::Cardinality,
};

#[test]
fn typedb_profiles_materialize_omitted_interface_cardinalities() {
    let bounded_to_one = Cardinality::new(0, Some(1)).expect("0..1 is a valid cardinality");
    let unbounded = Cardinality::new(0, None).expect("0..unbounded is a valid cardinality");

    for profile_id in ["typedb-3.11.5/v1", "typedb-3.12.1/v1"] {
        let profile_id =
            SemanticProfileId::new(profile_id.to_owned()).expect("profile ID is valid");
        let profile = SemanticProfile::resolve(&profile_id).expect("profile is supported");

        assert_eq!(
            profile.default_cardinality(InterfaceKind::Owns),
            bounded_to_one,
            "{profile_id:?} owns",
        );
        assert_eq!(
            profile.default_cardinality(InterfaceKind::Relates),
            bounded_to_one,
            "{profile_id:?} relates",
        );
        assert_eq!(
            profile.default_cardinality(InterfaceKind::Plays),
            unbounded,
            "{profile_id:?} plays",
        );
    }
}
