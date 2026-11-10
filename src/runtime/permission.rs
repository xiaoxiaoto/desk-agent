#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionError {
    pub missing_scope: String,
}

/// Enforces deny-by-default checks using only explicitly supplied scopes.
pub fn require_scope(granted: &[String], required: &str) -> Result<(), PermissionError> {
    if granted
        .iter()
        .any(|scope| scope == "admin" || scope == required)
    {
        Ok(())
    } else {
        Err(PermissionError {
            missing_scope: required.to_owned(),
        })
    }
}
