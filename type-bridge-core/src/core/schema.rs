use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSchema {
    pub entities: HashMap<String, EntityType>,
    pub relations: HashMap<String, RelationType>,
    pub attributes: HashMap<String, AttributeType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityType {
    pub name: String,
    pub parent: Option<String>,
    pub owns: Vec<String>,
    pub plays: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationType {
    pub name: String,
    pub parent: Option<String>,
    pub relates: Vec<String>,
    pub owns: Vec<String>,
    pub plays: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeType {
    pub name: String,
    pub value_type: String,
    pub parent: Option<String>,
}

impl TypeSchema {
    pub fn new() -> Self {
        TypeSchema {
            entities: HashMap::new(),
            relations: HashMap::new(),
            attributes: HashMap::new(),
        }
    }
}
