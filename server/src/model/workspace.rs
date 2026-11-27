//! Workspaces: separate spaces for runbooks, runs, vault secrets and hosts, each with its
//! own members and their roles. Records from before workspaces belong to `default`.

use serde::{Deserialize, Serialize};

/// The workspace that existed before there were workspaces, and that requests without a
/// `workspace` use. It can't be deleted.
pub const DEFAULT_WORKSPACE: &str = "default";

/// Serde default for records written before workspaces.
pub fn default_workspace() -> String {
    DEFAULT_WORKSPACE.into()
}

/// Serde default for hosts enrolled before workspaces.
pub fn default_workspaces() -> Vec<String> {
    vec![DEFAULT_WORKSPACE.into()]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// The id, used in the API (`?workspace=ops`): it never changes.
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: u64,
}

/// 1 to 40 lowercase letters, digits and dashes, starting with a letter or digit.
/// Returns the slug trimmed and lowercased.
pub fn validate_slug(slug: &str) -> Result<String, String> {
    let slug = slug.trim().to_lowercase();
    let ok = !slug.is_empty()
        && slug.len() <= 40
        && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && slug.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(slug)
    } else {
        Err("A workspace id is 1 to 40 lowercase letters, digits or dashes, starting with a letter or digit, like \"payments\".".into())
    }
}

pub fn validate_workspace_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("A workspace name is 1 to 60 characters.".into());
    }
    Ok(name)
}

pub fn validate_workspace_description(description: &str) -> Result<String, String> {
    let description = description.trim().to_string();
    if description.chars().count() > 300 {
        return Err("A workspace description is at most 300 characters.".into());
    }
    Ok(description)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_checked() {
        assert_eq!(validate_slug(" Payments ").unwrap(), "payments");
        assert_eq!(validate_slug("team-2").unwrap(), "team-2");
        for bad in ["", "-x", "a b", "a_b", "a/b", "é", &"a".repeat(41)] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
        assert!(validate_workspace_name(" ").is_err());
        assert_eq!(validate_workspace_name(" Ops team ").unwrap(), "Ops team");
    }
}
