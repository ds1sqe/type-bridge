// Port legacy response fixtures to native typed provider evidence.
use super::native_batch_answer_item::{AnswerItem, QueryResult};
use serde_json::{Value, json};

fn scalar(value: &Value) -> Value {
    let value = value.get("value").unwrap_or(value);
    let value = if let Some(value) = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    {
        type_bridge_contract::value::CanonicalValue::Long(value)
    } else {
        type_bridge_contract::value::CanonicalValue::String(
            type_bridge_contract::value::CanonicalString::new(value.as_str().unwrap_or(""))
                .unwrap(),
        )
    };
    json!({"kind":"value", "value":value})
}

fn attributes(value: &mut Value) {
    if let Some(fields) = value.as_object_mut() {
        for (name, value) in fields {
            if let Some(values) = value.as_array() {
                let values: Vec<_> = values
                    .iter()
                    .map(|value| value.get("value").unwrap_or(value).clone())
                    .collect();
                *value = if matches!(
                    name.as_str(),
                    "identifier" | "name" | "position" | "email" | "text"
                ) {
                    values.into_iter().next().unwrap_or(Value::Null)
                } else {
                    values.into()
                };
            }
        }
    }
}

fn relation(label: &str) -> bool {
    matches!(
        label,
        "membership"
            | "assignment"
            | "engagement"
            | "employment"
            | "event"
            | "pair"
            | "tagging"
            | "collaboration"
            | "gathering"
            | "container"
    )
}

#[allow(dead_code)] // Shared by SDK materializing mocks and ORM streaming mocks.
pub fn result(source: &str, result: QueryResult) -> QueryResult {
    if let QueryResult::Rows(mut rows) = result {
        if source.contains("reduce $count1 = count") {
            for row in &mut rows {
                if let Some(value) = row.get("$count").and_then(Value::as_i64) {
                    *row = json!({"count1": {"kind":"value", "value":type_bridge_contract::value::CanonicalValue::Long(value)}});
                }
            }
        }
        return QueryResult::Rows(rows);
    }
    let QueryResult::Documents(documents) = result else {
        return result;
    };
    if source.contains("select $iid1;") {
        return QueryResult::Rows(documents.into_iter().map(|document| json!({"iid1":{"kind":"value", "value":type_bridge_contract::value::CanonicalValue::String(type_bridge_contract::value::CanonicalString::new(document["iid"].as_str().unwrap()).unwrap())}})).collect());
    }
    if !source.contains("fetch") && source.ends_with("select $root0;") {
        let label = source
            .split("$root0 isa!")
            .nth(1)
            .or_else(|| source.split("$root0 isa").nth(1))
            .unwrap()
            .trim()
            .split([';', ',', ' '])
            .next()
            .unwrap();
        return QueryResult::Rows(documents.into_iter().map(|document| {
            let concrete = document.get("_type").and_then(Value::as_str).unwrap_or(label);
            let iid = document.get("_iid").or_else(|| document.get("iid")).cloned().unwrap_or(Value::Null);
            json!({"root0":{"kind":"thing", "iid":iid, "type_id":{"kind":if relation(concrete) {"relation"} else {"entity"},"label":concrete}}})
        }).collect());
    }
    if !source.contains("fetch") && source.contains("select $thing0;") {
        let label = source
            .split("$thing0 isa")
            .nth(1)
            .unwrap()
            .trim_start_matches('!')
            .trim()
            .split([';', ',', ' '])
            .next()
            .unwrap();
        return QueryResult::Rows(documents.into_iter().map(|document| json!({"thing0":{"kind":"thing", "iid":document["iid"], "type_id":{"kind":"entity", "label":label}}})).collect());
    }
    let native =
        source.contains("$ordinal0") || source.contains("$ordinal1") || source.contains("$root0");
    if !native {
        return QueryResult::Documents(documents);
    }
    if source.contains("fetch") {
        QueryResult::Documents(
            documents
                .into_iter()
                .map(|document| match item(source, document) {
                    AnswerItem::Document(document) => document,
                    _ => panic!("native fetch fixture"),
                })
                .collect(),
        )
    } else {
        QueryResult::Rows(
            documents
                .into_iter()
                .map(|document| match item(source, document) {
                    AnswerItem::Row(row) => row,
                    _ => panic!("native row fixture"),
                })
                .collect(),
        )
    }
}

pub fn item(source: &str, mut document: Value) -> AnswerItem {
    let snapshot = source.contains("$root0")
        && !source.contains("$ordinal1")
        && !source.contains("\"ordinal\"")
        && (source.contains("fetch") || source.contains("delete"));
    if !snapshot && !source.contains("$ordinal0") && !source.contains("$ordinal1") {
        return AnswerItem::Document(document);
    }
    if snapshot {
        document
            .as_object_mut()
            .expect("snapshot fixture object")
            .remove("ordinal");
    }
    if source.contains("fetch") {
        if source.contains("\"_role_0_players\"")
            && document.get("role_players").is_none()
            && document.get("_role_0_iid").is_some()
        {
            let mut players = Vec::new();
            for index in 0..32 {
                let Some(iid) = document
                    .as_object_mut()
                    .unwrap()
                    .remove(&format!("_role_{index}_iid"))
                else {
                    break;
                };
                let kind = document
                    .as_object_mut()
                    .unwrap()
                    .remove(&format!("_role_{index}_type"))
                    .unwrap_or(Value::Null);
                let attributes = document
                    .as_object_mut()
                    .unwrap()
                    .remove(&format!("_role_{index}_attributes"))
                    .unwrap_or(json!({}));
                let variable = format!("PlanFetchRole{index}");
                let marker = format!(": ${variable})");
                let role = source
                    .split(&marker)
                    .next()
                    .unwrap()
                    .rsplit('(')
                    .next()
                    .unwrap()
                    .trim();
                if !iid.is_null() {
                    players.push(
                        json!({"role":role,"iid":iid,"type_name":kind,"attributes":attributes}),
                    );
                }
            }
            document["role_players"] = players.into();
        }
        let players = document
            .as_object_mut()
            .and_then(|object| object.remove("role_players"));
        if let Some(players) = players {
            let players = players.as_array().expect("legacy role fixture list");
            let variable = if source.contains("$root0") {
                "$root0"
            } else {
                "$thing1"
            };
            let exact = format!("{variable} isa! ");
            let inclusive = format!("{variable} isa ");
            let relation = source
                .split(&exact)
                .skip(1)
                .chain(source.split(&inclusive).skip(1))
                .filter_map(|value| value.split([';', ',', ' ']).next())
                .find(|label| !label.starts_with('$'))
                .unwrap_or("");
            if players.iter().any(|player| {
                player["role"].as_str().is_some_and(|role| {
                    role.contains(':') && !role.starts_with(&format!("{relation}:"))
                })
            }) {
                document["_role_99_players"] = json!([]);
            }
            for index in 0..32 {
                let key = format!("_role_{index}_players");
                let iid_key = format!("_role_{index}_iid");
                let collected = source.contains(&format!("\"{key}\""));
                if !collected && !source.contains(&format!("\"{iid_key}\"")) {
                    break;
                }
                let variable = format!("PlanFetchRole{index}");
                let marker = format!(": ${variable})");
                let prefix = source
                    .split(&marker)
                    .next()
                    .expect("role variable in native fetch");
                let role = prefix.rsplit('(').next().expect("native role label").trim();
                let selected = players
                    .iter()
                    .filter(|player| {
                        player["role"]
                            .as_str()
                            .is_some_and(|name| name.rsplit(':').next() == Some(role))
                    })
                    .collect::<Vec<_>>();
                if collected {
                    document[&key] = selected.into_iter().map(|player| {
                        let mut attributes_value = player["attributes"].clone(); attributes(&mut attributes_value);
                        json!({"_iid":player["iid"], "_type":player["type_name"], "attributes":attributes_value})
                    }).collect::<Vec<_>>().into();
                } else {
                    assert!(
                        selected.len() <= 1,
                        "uncollected role fixtures expand in separate documents"
                    );
                    let player = selected.first().copied();
                    document[&iid_key] = player.map_or(Value::Null, |player| player["iid"].clone());
                    document[format!("_role_{index}_type")] =
                        player.map_or(Value::Null, |player| player["type_name"].clone());
                    let mut attributes_value =
                        player.map_or(Value::Null, |player| player["attributes"].clone());
                    attributes(&mut attributes_value);
                    document[format!("_role_{index}_attributes")] = attributes_value;
                }
            }
        }
        if let Some(values) = document.get_mut("attributes") {
            attributes(values);
        }
        return AnswerItem::Document(document);
    }
    if source.contains("$root0") && source.contains("$ordinal1") && !source.contains("fetch") {
        let label = document
            .get("_type")
            .and_then(Value::as_str)
            .unwrap_or("person");
        return AnswerItem::Row(json!({
            "ordinal1": scalar(&document["ordinal"]),
            "root0": {"kind":"thing", "iid":document["iid"], "type_id":{"kind":if relation(label) {"relation"} else {"entity"}, "label":label}},
        }));
    }
    if snapshot {
        return AnswerItem::Row(json!({}));
    }
    if source.contains("$kind0") {
        let label = document["type"].as_str().unwrap_or("person");
        return AnswerItem::Row(
            json!({"kind0":scalar(&document["kind"]), "ordinal1":scalar(&document["ordinal"]), "reference2":scalar(&document["reference_ordinal"]), "thing3":{"kind":"thing", "iid": document["iid"], "type_id":{"kind":if relation(label) {"relation"} else {"entity"}, "label":label}}}),
        );
    }
    let mut row = json!({"ordinal0":scalar(&document["ordinal"])});
    if source.ends_with("select $ordinal0, $thing1;") {
        let after = source.split("$thing1 isa").nth(1).unwrap_or(" person;");
        let label = after
            .trim_start_matches('!')
            .trim()
            .split([';', ',', ' '])
            .next()
            .unwrap();
        if let Some(iid) = document.get("iid") {
            row["thing1"] = json!({"kind":"thing", "iid":iid, "type_id":{"kind":if relation(label) {"relation"} else {"entity"}, "label":label}});
        } else {
            row["thing1"] = json!({"kind":"absent"});
        }
    }
    AnswerItem::Row(row)
}
