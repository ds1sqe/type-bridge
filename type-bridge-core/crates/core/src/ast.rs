use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Value {
    Literal(LiteralValue),
    FunctionCall(FunctionCallValue),
    Arithmetic(ArithmeticValue),
    Variable(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiteralValue {
    pub value: serde_json::Value,
    pub value_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallValue {
    pub function: String,
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArithmeticValue {
    pub left: Box<Value>,
    pub operator: String,
    pub right: Box<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePlayer {
    pub role: String,
    pub player_var: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Constraint {
    Iid(String),
    Has {
        attr_name: String,
        value: Value,
    },
    Isa {
        type_name: String,
        strict: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Pattern {
    Entity {
        variable: String,
        type_name: String,
        constraints: Vec<Constraint>,
        is_strict: bool,
    },
    Relation {
        variable: String,
        type_name: String,
        role_players: Vec<RolePlayer>,
        constraints: Vec<Constraint>,
    },
    SubType {
        variable: String,
        parent_type: String,
    },
    Attribute {
        variable: String,
        type_name: String,
        value: Option<Value>,
    },
    Has {
        thing_var: String,
        attr_type: String,
        attr_var: String,
    },
    ValueComparison {
        var: String,
        operator: String,
        value: Value,
    },
    Not(Vec<Pattern>),
    Or(Vec<Vec<Pattern>>),
    Iid {
        variable: String,
        iid: String,
    },
    Raw(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Statement {
    Has {
        subject_var: String,
        attr_name: String,
        value: Value,
    },
    Isa {
        variable: String,
        type_name: String,
    },
    Relation {
        variable: String,
        type_name: String,
        role_players: Vec<RolePlayer>,
        include_variable: bool,
        attributes: Vec<Statement>, // Should be HasStatement but recursive enum needs care
    },
    DeleteThing(String),
    Raw(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Clause {
    Match(Vec<Pattern>),
    MatchLet(Vec<LetAssignment>),
    Insert(Vec<Statement>),
    Delete(Vec<Statement>),
    Update(Vec<Statement>),
    Fetch(Vec<FetchItem>),
    Reduce {
        assignments: Vec<ReduceAssignment>,
        group_by: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LetAssignment {
    pub variables: Vec<String>,
    pub expression: Value,
    pub is_stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FetchItem {
    Attribute { key: String, var: String, attr_name: String },
    Variable { key: String, var: String },
    AttributeList { key: String, var: String, attr_name: String },
    Function { key: String, func_name: String, var: String },
    Wildcard { key: String, var: String },
    NestedWildcard { key: String, var: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReduceAssignment {
    pub variable: String,
    pub expression: Value,
}
