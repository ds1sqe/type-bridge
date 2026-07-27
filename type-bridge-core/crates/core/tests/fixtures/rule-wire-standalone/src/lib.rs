#[cfg(test)]
mod tests {
    use serde::Deserialize;

    mod released_wire {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../src/validation_rule_wire.rs"
        ));
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", content = "data")]
    enum RuleTypeWire {
        Range {
            min: Option<released_wire::ReleasedF64>,
            max: Option<released_wire::ReleasedF64>,
        },
    }

    #[test]
    fn primitive_number_representation_preserves_released_contract() {
        let range: RuleTypeWire =
            serde_json::from_str(r#"{"data":{"min":-1.25,"max":2.5},"type":"Range"}"#)
                .expect("content-first default-serde range");
        match range {
            RuleTypeWire::Range { min, max } => {
                assert_eq!(min.map(released_wire::ReleasedF64::into_inner), Some(-1.25));
                assert_eq!(max.map(released_wire::ReleasedF64::into_inner), Some(2.5));
            }
        }

        let string_error = serde_json::from_str::<RuleTypeWire>(
            r#"{"type":"Range","data":{"min":"zero","max":1}}"#,
        )
        .expect_err("string range bound")
        .to_string();
        assert_eq!(
            string_error,
            "invalid type: string \"zero\", expected f64 at line 1 column 36",
        );

        let overflow_error = serde_json::from_str::<RuleTypeWire>(
            r#"{"type":"Range","data":{"min":1e400,"max":1}}"#,
        )
        .expect_err("overflowing range bound")
        .to_string();
        assert_eq!(overflow_error, "number out of range at line 1 column 35",);

        let content_first = serde_json::from_str::<RuleTypeWire>(
            r#"{"data":{"min":"zero","max":1},"type":"Range"}"#,
        )
        .expect_err("content-first string range bound")
        .to_string();
        assert_eq!(
            content_first,
            "invalid type: string \"zero\", expected f64 at line 1 column 46"
        );
    }
}
