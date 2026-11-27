//! How people sign in: passwords, and one OIDC provider set up by a server admin.

use serde::{Deserialize, Serialize};

use super::{SHARED_PREFIX, validate_secret_ref};

/// Settings > Sign-in. Stored as one record; missing means passwords only, for everyone.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInSettings {
    /// The OIDC provider, if one is set up.
    #[serde(default)]
    pub oidc: Option<OidcSettings>,
    /// Only server admins may sign in with a password; everyone else uses the provider.
    /// Server admins keep a way in if the provider is down.
    #[serde(default)]
    pub passwords_for_server_admins_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcSettings {
    /// Turned off, the settings stay but the sign-in button goes away.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The button says "Sign in with <label>".
    pub label: String,
    /// The provider's issuer URL; its settings are read from
    /// `<issuer>/.well-known/openid-configuration`.
    pub issuer: String,
    pub client_id: String,
    /// The client secret: a shared secret, like `shared/oidc/CLIENT_SECRET`.
    pub client_secret: String,
    /// Link a user by the email a server admin set on them, when the provider says the
    /// email is verified.
    #[serde(default = "yes")]
    pub link_by_email: bool,
    /// Create an account on first sign-in, for these email domains. The new account is in
    /// no workspace until a workspace Admin adds it.
    #[serde(default)]
    pub create_accounts: bool,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Some providers (Entra ID) send no `email_verified`; trust their email anyway.
    #[serde(default)]
    pub trust_unverified_email: bool,
}

fn yes() -> bool {
    true
}

impl SignInSettings {
    pub fn validate(&mut self) -> Result<(), String> {
        if let Some(o) = &mut self.oidc {
            o.validate()?;
        }
        if self.passwords_for_server_admins_only && !self.oidc.as_ref().is_some_and(|o| o.enabled) {
            return Err("Allow passwords only for server admins once a sign-in provider is set up and on, or no one else could sign in.".into());
        }
        Ok(())
    }

    /// The provider, when it's on.
    pub fn provider(&self) -> Option<&OidcSettings> {
        self.oidc.as_ref().filter(|o| o.enabled)
    }
}

impl OidcSettings {
    fn validate(&mut self) -> Result<(), String> {
        self.label = self.label.trim().to_string();
        if self.label.is_empty() || self.label.chars().count() > 40 {
            return Err("The button label must be 1 to 40 characters, like Okta or Google.".into());
        }
        self.issuer = self.issuer.trim().trim_end_matches('/').to_string();
        let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"].iter().any(|p| self.issuer.starts_with(p));
        if !(self.issuer.starts_with("https://") || loopback) || self.issuer.len() > 500 || self.issuer.contains(char::is_whitespace) {
            return Err("The issuer must be an https URL, like https://example.okta.com or https://login.microsoftonline.com/<tenant>/v2.0.".into());
        }
        self.client_id = self.client_id.trim().to_string();
        if self.client_id.is_empty() || self.client_id.len() > 300 {
            return Err("Give the client ID the provider shows for this app.".into());
        }
        self.client_secret = self.client_secret.trim().to_string();
        if !self.client_secret.starts_with(SHARED_PREFIX) {
            return Err("The client secret must be a shared secret, like shared/oidc/CLIENT_SECRET. Add it under Vault > Shared first.".into());
        }
        validate_secret_ref(&self.client_secret)?;
        let mut domains = Vec::new();
        for d in &self.allowed_domains {
            let d = d.trim().trim_start_matches('@').to_lowercase();
            if d.is_empty() {
                continue;
            }
            let ok = d.len() <= 253 && d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') && !d.starts_with('.') && !d.ends_with('.');
            if !ok {
                return Err(format!("{d:?} is not an email domain, like example.com."));
            }
            if !domains.contains(&d) {
                domains.push(d);
            }
        }
        if domains.len() > 20 {
            return Err("At most 20 email domains.".into());
        }
        self.allowed_domains = domains;
        if self.create_accounts && self.allowed_domains.is_empty() {
            return Err("To create accounts on first sign-in, list the email domains they may come from, like example.com.".into());
        }
        Ok(())
    }

    /// Whether an email's domain is one of the allowed ones.
    pub fn domain_allowed(&self, email: &str) -> bool {
        email.rsplit_once('@').is_some_and(|(_, d)| self.allowed_domains.iter().any(|a| a == d))
    }
}
