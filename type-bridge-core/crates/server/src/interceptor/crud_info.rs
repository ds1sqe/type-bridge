/// CRUD operation metadata populated by CRUD handlers.
///
/// Non-CRUD requests (raw queries, validation) leave this as `Default`.
#[derive(Debug, Clone, Default)]
pub struct CrudInfo {
    /// Operation kind: "insert", "fetch", "update", "delete".
    pub operation: Option<String>,
    /// The TypeDB type name (e.g. "person", "employment").
    pub type_name: Option<String>,
    /// "entity" or "relation".
    pub type_kind: Option<String>,
    /// Names of attributes involved in the operation.
    pub attribute_names: Vec<String>,
    /// IID if the operation targets a specific instance.
    pub iid: Option<String>,
}

impl CrudInfo {
    /// Returns `true` if this context represents a CRUD operation.
    pub fn is_crud(&self) -> bool {
        self.operation.is_some()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_is_not_crud() {
        let info = CrudInfo::default();
        assert!(!info.is_crud());
        assert!(info.operation.is_none());
        assert!(info.type_name.is_none());
        assert!(info.type_kind.is_none());
        assert!(info.attribute_names.is_empty());
        assert!(info.iid.is_none());
    }

    #[test]
    fn is_crud_true_when_operation_set() {
        let info = CrudInfo {
            operation: Some("insert".to_string()),
            ..Default::default()
        };
        assert!(info.is_crud());
    }

    #[test]
    fn debug_impl() {
        let info = CrudInfo {
            operation: Some("fetch".to_string()),
            type_name: Some("person".to_string()),
            type_kind: Some("entity".to_string()),
            attribute_names: vec!["name".to_string()],
            iid: None,
        };
        let debug = format!("{:?}", info);
        assert!(debug.contains("fetch"));
        assert!(debug.contains("person"));
        assert!(debug.contains("entity"));
    }

    #[test]
    fn clone_impl() {
        let info = CrudInfo {
            operation: Some("delete".to_string()),
            type_name: Some("person".to_string()),
            type_kind: Some("entity".to_string()),
            attribute_names: vec!["name".to_string(), "age".to_string()],
            iid: Some("0xabc".to_string()),
        };
        let cloned = info.clone();
        assert_eq!(cloned.operation.as_deref(), Some("delete"));
        assert_eq!(cloned.type_name.as_deref(), Some("person"));
        assert_eq!(cloned.type_kind.as_deref(), Some("entity"));
        assert_eq!(cloned.attribute_names, vec!["name", "age"]);
        assert_eq!(cloned.iid.as_deref(), Some("0xabc"));
    }
}
