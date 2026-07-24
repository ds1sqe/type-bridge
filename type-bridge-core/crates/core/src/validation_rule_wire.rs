use serde::Deserialize;

// `serde` buffers the `data` member of an adjacently tagged enum before it
// knows which variant to construct. With serde_json's `arbitrary_precision`
// feature, buffered numbers use a private map representation rather than the
// primitive representation seen by `f64`. This wrapper accepts both forms but
// deliberately retains the released `f64` visitor's malformed-input wording.
#[derive(Debug)]
pub(crate) struct ReleasedF64(f64);

impl ReleasedF64 {
    pub(crate) fn into_inner(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReleasedF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ReleasedF64Visitor;

        impl<'de> serde::de::Visitor<'de> for ReleasedF64Visitor {
            type Value = ReleasedF64;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("f64")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(ReleasedF64(value as f64))
            }

            fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E> {
                Ok(ReleasedF64(value as f64))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(ReleasedF64(value as f64))
            }

            fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E> {
                Ok(ReleasedF64(value as f64))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value.is_finite() {
                    Ok(ReleasedF64(value))
                } else {
                    Err(E::custom("number out of range"))
                }
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let deserializer = serde::de::value::MapAccessDeserializer::new(map);
                let number = match serde_json::Number::deserialize(deserializer) {
                    Ok(number) => number,
                    Err(_) => {
                        return Err(serde::de::Error::invalid_type(
                            serde::de::Unexpected::Map,
                            &self,
                        ));
                    }
                };
                number
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(ReleasedF64)
                    .ok_or_else(|| serde::de::Error::custom("number out of range"))
            }
        }

        deserializer.deserialize_any(ReleasedF64Visitor)
    }
}
