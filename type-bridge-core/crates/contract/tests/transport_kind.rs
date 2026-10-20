//! Transport descriptions are a closed value-only vocabulary.
use type_bridge_contract::failure::TransportKind;

#[test]
fn transport_descriptions_round_trip_without_free_text() {
    for kind in [
        TransportKind::Timeout,
        TransportKind::Unavailable,
        TransportKind::Tls,
        TransportKind::Authentication,
        TransportKind::Closed,
        TransportKind::Other,
    ] {
        assert_eq!(
            TransportKind::from_description(kind.description()),
            Some(kind)
        );
        assert_eq!(
            TransportKind::from_description(&format!("{} SECRET_SENTINEL", kind.description())),
            None
        );
    }
    assert_eq!(
        TransportKind::from_description("SECRET_SENTINEL timeout"),
        None
    );
}
