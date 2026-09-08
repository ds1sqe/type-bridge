//! Share pinned dependency graphs while preserving each fixture's package record.

const OFFLINE: &str = include_str!("rust-offline/Cargo.lock");
const LIVE: &str = include_str!("rust-live/Cargo.lock");
const CODEC: &str = include_str!("rust-offline/codec.toml");
const PROJECTED_LIVE: &str = include_str!("rust-live/projected.toml");

const ADMINISTRATION: &str = include_str!("rust-live/administration.toml");

pub enum Consumer {
    Projected,
    Codec,
    ProjectedLive,
    ManagerLive,
    AdministrationLive,
}

impl Consumer {
    pub fn lock(&self) -> String {
        match self {
            Self::Projected => OFFLINE.to_owned(),
            Self::Codec => {
                replace_package(OFFLINE, "rust-projected-projection-parity", Some(CODEC))
            }
            Self::ManagerLive => LIVE.to_owned(),
            Self::AdministrationLive => replace_package(
                &replace_package(LIVE, "rust-manager-live", Some(ADMINISTRATION)),
                "type-bridge-generated-schema-foreign",
                None,
            ),
            Self::ProjectedLive => replace_package(
                &replace_package(LIVE, "rust-manager-live", Some(PROJECTED_LIVE)),
                "type-bridge-generated-schema-foreign",
                None,
            ),
        }
    }
}

fn replace_package(lock: &str, name: &str, replacement: Option<&str>) -> String {
    let mut sections = lock.split("\n[[package]]\n");
    let mut output = vec![sections.next().expect("lock has a header")];
    let marker = format!("name = \"{name}\"\n");
    let mut replaced = 0;
    for section in sections {
        if section.starts_with(&marker) {
            replaced += 1;
            output.extend(replacement);
        } else {
            output.push(section);
        }
    }
    assert_eq!(
        replaced, 1,
        "fixture package {name} must occur exactly once"
    );
    output.join("\n[[package]]\n")
}
