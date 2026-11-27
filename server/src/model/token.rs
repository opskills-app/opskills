//! API tokens.


use serde::{Deserialize, Serialize};



/// An API token for automation. Its secret is never stored, only the SHA-256.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub name: String,
    /// The user the token acts as.
    pub user: String,
    pub created_at: u64,
    /// None: never expires.
    pub expires_at: Option<u64>,
    /// Updated at most once a minute.
    pub last_used_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredApiToken {
    #[serde(flatten)]
    pub token: ApiToken,
    pub secret_hash: String,
}
