//! Twelve binding chain/star query against the runner-owned database.
use models::*;
use serde_json::json;
use type_bridge::value::Text;
use type_bridge::{
    ConnectionOptions, Database, PageOptions, ProjectedManagerComparison, RowsOptions, aggregate,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    let db = Database::connect(
        ConnectionOptions::new(
            std::env::var("TYPEDB_ADDRESS")?,
            std::env::var("TYPE_BRIDGE_COMPLEX_DATABASE")?,
        )
        .credentials("admin", "password")
        .http_port(std::env::var("TYPEDB_HTTP_PORT")?.parse()?),
    )
    .await?
    .with_schema(SCHEMA)?;
    if let Ok(raw) = std::env::var("TYPE_BRIDGE_COMPLEX_RAW_QUERY") {
        println!(
            "{}",
            db.execute_query(&raw, type_bridge::raw::TransactionKind::Read)
                .await?
        );
        db.close()?;
        return Ok(());
    }
    let mut exact_abstract = type_bridge::plan::PlanBuilder::new();
    exact_abstract.exact::<Party>();
    exact_abstract.count();
    let error = exact_abstract.build(&SCHEMA).unwrap_err();
    assert_eq!(error.code(), Some("query_plan_abstract_exact_read"));
    let mut abstract_subtypes = type_bridge::plan::PlanBuilder::new();
    abstract_subtypes.subtypes::<Party>();
    abstract_subtypes.count();
    let abstract_plan = abstract_subtypes.build(&SCHEMA)?;
    let mut transaction = db.transaction(type_bridge::staged::TransactionOptions::default());
    let answer = transaction.stage(&abstract_plan)?;
    transaction.commit().await?;
    assert_eq!(
        answer.result()?,
        type_bridge::staged::PlanAnswer::Count(db.entities::<Party>().subtypes().count().await?)
    );
    transaction.close()?;
    let mut s = db.query()?;
    let p = s.subtypes::<Person>()?;
    let c = s.exact::<Company>()?;
    let d = s.exact::<Department>()?;
    let j = s.exact::<Project>()?;
    let t = s.exact::<Task>()?;
    let o = s.exact::<Office>()?;
    let e = s.exact::<Employment>()?;
    let m = s.exact::<Membership>()?;
    let a = s.exact::<Assignment>()?;
    let x = s.exact::<ProjectTask>()?;
    let l = s.exact::<Located>()?;
    let z = s.exact::<CompanyOffice>()?;
    let abstract_binding = s.subtypes::<Party>()?;
    let size: u64 = std::env::var("TYPE_BRIDGE_COMPLEX_SIZE")?.parse()?;
    let limit = size + 1;
    let width: u32 = std::env::var("TYPE_BRIDGE_COMPLEX_WIDTH")
        .unwrap_or_else(|_| "12".into())
        .parse()?;
    let mut query = s.query((p, e))?.where_all([
        e.role(EmploymentType::worker).links(p),
        p.field(PersonType::tag).eq(Tag::new("a")?),
        p.field(PersonType::tag).eq(Tag::new("z")?),
        p.field(PersonType::score).ge(20_i64),
        e.field(EmploymentType::note).is_present(),
    ])?;
    if width < 12 {
        query = query.where_all([e
            .field(EmploymentType::note)
            .starts_with(Text::new("job-")?)])?;
        query = query.where_all([p
            .field(PersonType::identifier)
            .eq(Identifier::new("P0002")?)
            .or(p
                .field(PersonType::identifier)
                .eq(Identifier::new("P0004")?))
            .or(p
                .field(PersonType::identifier)
                .eq(Identifier::new("P0008")?))])?;
    }
    if width >= 4 {
        query = query.matching(c)?.matching(m)?.where_all([
            e.role(EmploymentType::employer).links(c),
            m.role(MembershipType::member).links(p),
            c.field(CompanyType::identifier)
                .starts_with(Text::new("C")?),
        ])?;
    }
    if width >= 6 {
        query = query.matching(d)?.matching(l)?.where_all([
            m.role(MembershipType::department).links(d),
            l.role(LocatedType::department).links(d),
            d.field(DepartmentType::blocked).eq(true).not(),
        ])?;
    }
    if width >= 8 {
        query = query.matching(o)?.matching(z)?.where_all([
            l.role(LocatedType::office).links(o),
            z.role(CompanyOfficeType::company).links(c),
            z.role(CompanyOfficeType::office).links(o),
            o.field(OfficeType::site).contains(Text::new("north")?),
        ])?;
    }
    if width >= 10 {
        query = query.matching(a)?.matching(j)?.where_all([
            a.role(AssignmentType::worker).links(p),
            a.role(AssignmentType::project).links(j),
            j.field(ProjectType::priority).ge(1_i64),
            p.field(PersonType::score)
                .ge_field(j.field(ProjectType::priority)),
        ])?;
    }
    if width >= 12 {
        query = query.matching(x)?.matching(t)?.where_all([
            x.role(ProjectTaskType::project).links(j),
            x.role(ProjectTaskType::task).links(t),
            t.field(TaskType::done).eq(false).or(t
                .field(TaskType::identifier)
                .starts_with(Text::new("T0008")?)),
        ])?;
    }
    let mut root_query = s.query(p)?.matching(e)?.where_all([
        e.role(EmploymentType::worker).links(p),
        p.field(PersonType::tag).eq(Tag::new("a")?),
        p.field(PersonType::tag).eq(Tag::new("z")?),
        p.field(PersonType::score).ge(20_i64),
        e.field(EmploymentType::note).is_present(),
    ])?;
    if width < 12 {
        root_query = root_query.where_all([e
            .field(EmploymentType::note)
            .starts_with(Text::new("job-")?)])?;
        root_query = root_query.where_all([p
            .field(PersonType::identifier)
            .eq(Identifier::new("P0002")?)
            .or(p
                .field(PersonType::identifier)
                .eq(Identifier::new("P0004")?))
            .or(p
                .field(PersonType::identifier)
                .eq(Identifier::new("P0008")?))])?;
    }
    if width >= 4 {
        root_query = root_query.matching(c)?.matching(m)?.where_all([
            e.role(EmploymentType::employer).links(c),
            m.role(MembershipType::member).links(p),
            c.field(CompanyType::identifier)
                .starts_with(Text::new("C")?),
        ])?;
    }
    if width >= 6 {
        root_query = root_query.matching(d)?.matching(l)?.where_all([
            m.role(MembershipType::department).links(d),
            l.role(LocatedType::department).links(d),
            d.field(DepartmentType::blocked).eq(true).not(),
        ])?;
    }
    if width >= 8 {
        root_query = root_query.matching(o)?.matching(z)?.where_all([
            l.role(LocatedType::office).links(o),
            z.role(CompanyOfficeType::company).links(c),
            z.role(CompanyOfficeType::office).links(o),
            o.field(OfficeType::site).contains(Text::new("north")?),
        ])?;
    }
    if width >= 10 {
        root_query = root_query.matching(a)?.matching(j)?.where_all([
            a.role(AssignmentType::worker).links(p),
            a.role(AssignmentType::project).links(j),
            j.field(ProjectType::priority).ge(1_i64),
            p.field(PersonType::score)
                .ge_field(j.field(ProjectType::priority)),
        ])?;
    }
    if width >= 12 {
        root_query = root_query.matching(x)?.matching(t)?.where_all([
            x.role(ProjectTaskType::project).links(j),
            x.role(ProjectTaskType::task).links(t),
            t.field(TaskType::done).eq(false).or(t
                .field(TaskType::identifier)
                .starts_with(Text::new("T0008")?)),
        ])?;
    }
    let sort = p.field(PersonType::identifier).asc();
    let rows = query
        .rows(
            RowsOptions::new(limit)
                .sort(sort.clone())
                .sort(e.field(EmploymentType::note).asc()),
        )
        .await?;
    let first = query.first(sort).await?;
    let encode = |pair: &(PersonFamily, Employment)| {
        json!([pair.0.identifier().value(), pair.1.note().unwrap().value()])
    };
    let page = root_query
        .page_by(
            p,
            PageOptions::new(1)
                .offset(1)
                .sort(p.field(PersonType::identifier).asc())
                .include_total(true),
        )
        .await?;
    assert_eq!(
        page.items()
            .iter()
            .map(|item| item.identifier().value())
            .collect::<Vec<_>>(),
        rows.iter()
            .skip(1)
            .take(1)
            .map(|row| row.0.identifier().value())
            .collect::<Vec<_>>()
    );
    assert_eq!(page.total(), Some(rows.len() as u64));
    let offset_rows = query
        .rows(
            RowsOptions::new(1)
                .offset(1)
                .sort(p.field(PersonType::identifier).asc()),
        )
        .await?;
    assert_eq!(
        offset_rows.iter().map(encode).collect::<Vec<_>>(),
        rows.iter().skip(1).take(1).map(encode).collect::<Vec<_>>()
    );
    let grouped = root_query
        .group_by_field(p.field(PersonType::identifier))?
        .reduce((aggregate::count(),))
        .await?;
    let mut actual_groups = grouped
        .iter()
        .map(|(key, (count,))| (key.value(), *count))
        .collect::<Vec<_>>();
    let mut expected_groups = rows
        .iter()
        .map(|row| (row.0.identifier().value(), 1_u64))
        .collect::<Vec<_>>();
    actual_groups.sort();
    expected_groups.sort();
    assert_eq!(actual_groups, expected_groups);
    let abstract_count = s
        .query(abstract_binding)?
        .rows(RowsOptions::new(limit))
        .await?
        .len();
    assert_eq!(
        abstract_count,
        db.entities::<Party>().subtypes().all().await?.len()
    );
    let tagged = db
        .entities::<Party>()
        .subtypes()
        .filter()?
        .where_(
            PartyType::tag,
            ProjectedManagerComparison::Eq,
            &Tag::new("a")?,
        )?
        .where_(
            PartyType::tag,
            ProjectedManagerComparison::Eq,
            &Tag::new("z")?,
        )?;
    assert_eq!(tagged.all().await?.len() as u64, tagged.count().await?);
    assert_eq!(tagged.exists().await?, tagged.count().await? > 0);
    assert_eq!(
        tagged
            .where_(
                PartyType::identifier,
                ProjectedManagerComparison::Eq,
                &Identifier::new("P0002")?
            )?
            .first()
            .await?
            .is_some(),
        size >= 2
    );
    println!(
        "{}",
        json!({
            "page_id": page.items().first().map(|row| row.identifier().value()),
            "groups": actual_groups,
            "rows": rows.iter().map(encode).collect::<Vec<_>>(), "first": first.as_ref().map(encode),
            "count": query.count_by(p).await?, "exists": query.exists_by(p).await?,
            "abstract_count": abstract_count, "tagged_count": tagged.count().await?,
        })
    );
    db.close()?;
    Ok(())
}
