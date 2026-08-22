use generated::*;
use type_bridge::__codegen::{
    CanonicalDouble, Date, DateTime, DateTimeTz, Decimal, Duration, HydratedPlayer, HydratedRow,
    IntoEncodedScalar, materialize_model_for_test,
};
use type_bridge::{AnswerCancellation, CanonicalCodecLimits, CanonicalCodecOptions, Error};

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn publish_provider_free_corpus(records: &[Vec<u8>; 9], archive: &[u8]) -> std::io::Result<()> {
    let Some(path) = std::env::var_os("TYPE_BRIDGE_WORKFORCE_V5_CORPUS") else {
        return Ok(());
    };
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Workforce V5 corpus path must be absolute",
        ));
    }
    let record_b64 = records
        .iter()
        .map(|record| format!("\"{}\"", base64(record)))
        .collect::<Vec<_>>()
        .join(",");
    let payload = format!(
        "{{\"archive_b64\":\"{}\",\"binding\":\"rust\",\"format\":\"typebridge.workforce-v5-provider-free-corpus/v1\",\"record_b64\":[{}]}}",
        base64(archive),
        record_b64,
    );
    use std::io::Write as _;
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    output.write_all(payload.as_bytes())?;
    output.sync_all()
}

fn expect_code<T>(result: type_bridge::Result<T>, expected: &str) -> Error {
    let Err(error) = result else {
        panic!("controlled canonical operation must fail");
    };
    assert_eq!(error.code(), Some(expected));
    error
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let integer_key = SCHEMA.encode_attribute(RobotId::new(9_007_199_254_740_993_i64)?)?;
    let stats = SCHEMA.encode_struct(PlayerStats::try_new(Some("stable".to_owned()), 3))?;

    let identifier = Identifier::new("person-v5")?;
    let score = Score::new(42)?;
    let val_double = ValDouble::new(CanonicalDouble::try_new(-0.0)?)?;
    let val_decimal = ValDecimal::new(Decimal::try_new("123.45")?)?;
    let val_bool = ValBool::new(true)?;
    let val_date = ValDate::new(Date::try_new("2026-08-22")?)?;
    let val_datetime = ValDatetime::new(DateTime::try_new("2026-08-22T12:34:56")?)?;
    let val_datetime_tz =
        ValDatetimeTz::new(DateTimeTz::try_new("2026-08-22T12:34:56Z")?)?;
    let val_duration = ValDuration::new(Duration::try_new("P1D")?)?;
    let val_constrained = ValConstrained::new(50)?;
    let person_create_value = PersonCreate::new(
        Vec::<Aliases>::new(),
        None,
        identifier.clone(),
        None,
        score.clone(),
        Some(ScoreGte::new(7)?),
        val_bool.clone(),
        val_constrained.clone(),
        val_date.clone(),
        val_datetime.clone(),
        val_datetime_tz.clone(),
        val_decimal.clone(),
        val_double.clone(),
        val_duration.clone(),
    )?;
    let person_create = SCHEMA.encode_create(person_create_value)?;

    let identifier_scalar = identifier.value().into_encoded_scalar();
    let person_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "0x501".to_owned(),
        vec![
            (PersonType::identifier.owns_id_json(), vec![identifier_scalar.clone()]),
            (PersonType::score.owns_id_json(), vec![score.value().into_encoded_scalar()]),
            (PersonType::score__gte.owns_id_json(), vec![7_i64.into_encoded_scalar()]),
            (PersonType::val_bool.owns_id_json(), vec![val_bool.value().into_encoded_scalar()]),
            (PersonType::val_constrained.owns_id_json(), vec![val_constrained.value().into_encoded_scalar()]),
            (PersonType::val_date.owns_id_json(), vec![val_date.value().into_encoded_scalar()]),
            (PersonType::val_datetime.owns_id_json(), vec![val_datetime.value().into_encoded_scalar()]),
            (PersonType::val_datetime_tz.owns_id_json(), vec![val_datetime_tz.value().into_encoded_scalar()]),
            (PersonType::val_decimal.owns_id_json(), vec![val_decimal.value().into_encoded_scalar()]),
            (PersonType::val_double.owns_id_json(), vec![val_double.value().into_encoded_scalar()]),
            (PersonType::val_duration.owns_id_json(), vec![val_duration.value().into_encoded_scalar()]),
        ],
        vec![],
    );
    let person: Person = materialize_model_for_test(&person_row)?;
    let person_snapshot = SCHEMA.encode_snapshot(person.clone())?;

    let robot_ref = generated::RobotRef::from_key(RobotId::new(9_007_199_254_740_993_i64)?)?;
    let membership = SCHEMA.encode_create(MembershipCreate::new(
        MembershipMemberRef::Robot(robot_ref.clone()),
    )?)?;
    let interaction = SCHEMA.encode_create(InteractionCreate::new(
        Identifier::new("interaction-v5")?,
        Some(Nickname::new("Ada")?),
        Some(InteractionActorRef::Robot(robot_ref)),
        PersonRef::from_key(identifier.clone())?,
    )?)?;
    let event_ref = EventRef::from_iid("0x700")?;
    let container = SCHEMA.encode_create(ContainerCreate::new(vec![event_ref.clone()])?)?;

    let player = HydratedPlayer::from_complete_row(person_row);
    let employment_row = HydratedRow::new(
        Employment::TYPE_ID_JSON,
        "0x801".to_owned(),
        vec![],
        vec![(EmploymentType::employee.role_id_json(), vec![player])],
    );
    let employment: Employment = materialize_model_for_test(&employment_row)?;
    let employment_snapshot = SCHEMA.encode_snapshot(employment)?;
    let player_reference = SCHEMA.encode_reference(event_ref)?;

    let records = [
        integer_key,
        stats,
        person_create,
        person_snapshot,
        membership,
        interaction,
        container,
        employment_snapshot,
        player_reference,
    ];
    let archive = SCHEMA.encode_archive(records.iter().map(Vec::as_slice))?;
    let decoded = SCHEMA.decode_archive(&archive)?;
    assert_eq!(decoded, records);
    let _: RobotId = SCHEMA.decode_attribute(&decoded[0])?;
    let _: PlayerStats = SCHEMA.decode_struct(&decoded[1])?;
    let _: PersonCreate = SCHEMA.decode_create(&decoded[2])?;
    let detached_person: Person = SCHEMA.decode_snapshot(&decoded[3])?;
    let _: MembershipCreate = SCHEMA.decode_create(&decoded[4])?;
    let _: InteractionCreate = SCHEMA.decode_create(&decoded[5])?;
    let _: ContainerCreate = SCHEMA.decode_create(&decoded[6])?;
    let detached_employment: Employment = SCHEMA.decode_snapshot(&decoded[7])?;
    let _: EventRef = SCHEMA.decode_reference(&decoded[8])?;
    assert_eq!(SCHEMA.encode_snapshot(detached_person)?, decoded[3]);
    assert_eq!(SCHEMA.encode_snapshot(detached_employment)?, decoded[7]);

    let cancellation = AnswerCancellation::default();
    cancellation.cancel();
    let cancelled = CanonicalCodecOptions::new().with_cancellation(cancellation);
    expect_code(
        SCHEMA.encode_archive_controlled(records.iter().map(Vec::as_slice), &cancelled),
        "projected_codec_cancelled",
    );
    let input_limited = CanonicalCodecOptions::new().with_limits(
        CanonicalCodecLimits::new().with_max_input_bytes(archive.len() - 1),
    );
    expect_code(
        SCHEMA.decode_archive_controlled(&archive, &input_limited),
        "projected_codec_input_limit",
    );
    let output_limited = CanonicalCodecOptions::new().with_limits(
        CanonicalCodecLimits::new().with_max_output_bytes(archive.len() - 1),
    );
    expect_code(
        SCHEMA.encode_archive_controlled(records.iter().map(Vec::as_slice), &output_limited),
        "projected_codec_output_limit",
    );
    let member_limited = CanonicalCodecOptions::new()
        .with_limits(CanonicalCodecLimits::new().with_max_records(records.len() - 1));
    expect_code(
        SCHEMA.encode_archive_controlled(records.iter().map(Vec::as_slice), &member_limited),
        "projected_codec_member_limit",
    );
    let depth_limited = CanonicalCodecOptions::new()
        .with_limits(CanonicalCodecLimits::new().with_max_depth(1));
    expect_code(
        SCHEMA.decode_archive_controlled(&archive, &depth_limited),
        "projected_codec_depth_limit",
    );
    let timed_out = CanonicalCodecOptions::new().with_timeout(std::time::Duration::ZERO);
    expect_code(
        SCHEMA.decode_archive_controlled(&archive, &timed_out),
        "projected_codec_deadline_exceeded",
    );
    publish_provider_free_corpus(&records, &archive)?;
    Ok(())
}
