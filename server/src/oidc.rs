//! Signing in with an OIDC provider: the authorization code flow with PKCE, a `state`
//! bound to the browser by a cookie, and a `nonce`. The ID token is checked against the
//! provider's published keys, issuer, audience, expiry and the nonce before anyone is
//! signed in. Nothing here trusts headers set by a proxy.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpRequest, HttpResponse, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, SyncHttpClient,
    TokenResponse, http,
};

use crate::model::OidcSettings;

/// The cookie that ties a sign-in in progress to the browser that started it.
pub const PENDING_COOKIE: &str = "opskills_oidc";
/// How long someone has at the provider before the sign-in must start again.
const PENDING_FOR: Duration = Duration::from_secs(600);
/// Sign-ins in progress kept at once; the oldest go first.
const MAX_PENDING: usize = 1000;
/// How long the provider's settings and keys are reused before asking again.
const DISCOVERY_FOR: Duration = Duration::from_secs(300);
const MAX_BODY: u64 = 1_000_000;

/// The HTTP client the library uses: ureq, 10 seconds, and no redirects, as the OAuth
/// security advice asks for these calls.
pub struct Http(ureq::Agent);

impl Default for Http {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).max_redirects(0).build().into();
        Http(agent)
    }
}

#[derive(Debug)]
pub struct HttpError(String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

impl SyncHttpClient for Http {
    type Error = HttpError;

    fn call(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut res = self.0.run(request).map_err(|e| HttpError(e.to_string()))?;
        let mut out = http::Response::builder().status(res.status());
        for (k, v) in res.headers() {
            out = out.header(k, v);
        }
        let body = res.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| HttpError(e.to_string()))?;
        out.body(body).map_err(|e| HttpError(e.to_string()))
    }
}

/// A sign-in between leaving for the provider and coming back.
struct Pending {
    state: String,
    nonce: Nonce,
    verifier: PkceCodeVerifier,
    metadata: CoreProviderMetadata,
    /// The settings it started with; a change in between makes it start again.
    issuer: String,
    client_id: String,
    started: Instant,
}

/// Who the provider says signed in, after the ID token checked out.
#[derive(Clone, Debug)]
pub struct ProviderUser {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub preferred_username: Option<String>,
    /// Google's Workspace domain claim.
    pub hosted_domain: Option<String>,
}

/// Sign-ins in progress and the provider's settings, in memory. A restart in the middle
/// of a sign-in just means signing in again.
#[derive(Default)]
pub struct OidcState {
    pending: Mutex<HashMap<String, Pending>>,
    discovery: Mutex<Option<(String, Instant, CoreProviderMetadata)>>,
    pub http: Http,
}

impl OidcState {
    /// The provider's settings and keys, from `<issuer>/.well-known/openid-configuration`.
    /// Blocking; reused for a few minutes.
    pub fn discover(&self, issuer: &str) -> Result<CoreProviderMetadata, String> {
        self.discover_with(issuer, false)
    }

    /// The same, always asking the provider (the settings page's test).
    pub fn discover_now(&self, issuer: &str) -> Result<CoreProviderMetadata, String> {
        self.discover_with(issuer, true)
    }

    fn discover_with(&self, issuer: &str, fresh: bool) -> Result<CoreProviderMetadata, String> {
        if !fresh
            && let Some((i, at, m)) = self.discovery.lock().unwrap().as_ref()
            && i == issuer
            && at.elapsed() < DISCOVERY_FOR
        {
            return Ok(m.clone());
        }
        let url = IssuerUrl::new(issuer.to_string()).map_err(|e| format!("the issuer URL is not valid: {e}"))?;
        let metadata = CoreProviderMetadata::discover(&url, &self.http).map_err(|e| format!("reading the provider's settings from {issuer} failed: {}", error_chain(&e)))?;
        *self.discovery.lock().unwrap() = Some((issuer.to_string(), Instant::now(), metadata.clone()));
        Ok(metadata)
    }

    /// Forget the provider's settings, after Settings > Sign-in changes.
    pub fn forget(&self) {
        *self.discovery.lock().unwrap() = None;
        self.pending.lock().unwrap().clear();
    }

    /// Start a sign-in: the URL to send the browser to, and the value of the cookie that
    /// must come back with it. Blocking.
    pub fn start(&self, settings: &OidcSettings, secret: &str, redirect: &str) -> Result<(String, String), String> {
        let metadata = self.discover(&settings.issuer)?;
        let client = client(&metadata, settings, secret, redirect)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = client
            .authorize_url(CoreAuthenticationFlow::AuthorizationCode, CsrfToken::new_random, Nonce::new_random)
            .add_scope(Scope::new("email".into()))
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge)
            .url();
        let id = crate::auth::new_token();
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, p| p.started.elapsed() < PENDING_FOR);
        if pending.len() >= MAX_PENDING
            && let Some(oldest) = pending.iter().min_by_key(|(_, p)| p.started).map(|(k, _)| k.clone())
        {
            pending.remove(&oldest);
        }
        pending.insert(
            id.clone(),
            Pending { state: state.secret().clone(), nonce, verifier, metadata, issuer: settings.issuer.clone(), client_id: settings.client_id.clone(), started: Instant::now() },
        );
        Ok((url.to_string(), id))
    }

    /// Finish a sign-in: trade the code for tokens and check the ID token. Each sign-in in
    /// progress can be finished once. Blocking.
    pub fn finish(&self, settings: &OidcSettings, secret: &str, redirect: &str, cookie: Option<&str>, state: &str, code: &str) -> Result<ProviderUser, String> {
        let pending = cookie.and_then(|id| self.pending.lock().unwrap().remove(id)).ok_or("this sign-in has expired or was started in another browser. Sign in again.")?;
        if pending.started.elapsed() > PENDING_FOR {
            return Err("this sign-in took too long. Sign in again.".into());
        }
        if !constant_time_eq(pending.state.as_bytes(), state.as_bytes()) {
            return Err("the sign-in came back with the wrong state. Sign in again.".into());
        }
        if pending.issuer != settings.issuer || pending.client_id != settings.client_id {
            return Err("the sign-in settings changed during this sign-in. Sign in again.".into());
        }
        let client = client(&pending.metadata, settings, secret, redirect)?;
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .map_err(|e| format!("the provider has no token endpoint: {e}"))?
            .set_pkce_verifier(pending.verifier)
            .request(&self.http)
            .map_err(|e| format!("the provider refused the sign-in code: {}", error_chain(&e)))?;
        let id_token = token.id_token().ok_or("the provider sent no ID token. Check that the app is set up for OpenID Connect.")?;
        let claims = id_token.claims(&client.id_token_verifier(), &pending.nonce).map_err(|e| format!("the ID token didn't check out: {}", error_chain(&e)))?;
        Ok(ProviderUser {
            issuer: claims.issuer().to_string(),
            subject: claims.subject().to_string(),
            email: claims.email().map(|e| e.to_string()),
            email_verified: claims.email_verified(),
            preferred_username: claims.preferred_username().map(|u| u.to_string()),
            hosted_domain: payload_claim(&id_token.to_string(), "hd"),
        })
    }
}

fn client(
    metadata: &CoreProviderMetadata,
    settings: &OidcSettings,
    secret: &str,
    redirect: &str,
) -> Result<CoreClient<openidconnect::EndpointSet, openidconnect::EndpointNotSet, openidconnect::EndpointNotSet, openidconnect::EndpointNotSet, openidconnect::EndpointMaybeSet, openidconnect::EndpointMaybeSet>, String>
{
    let redirect = RedirectUrl::new(redirect.to_string()).map_err(|e| format!("the redirect URL is not valid: {e}"))?;
    Ok(CoreClient::from_provider_metadata(metadata.clone(), ClientId::new(settings.client_id.clone()), Some(ClientSecret::new(secret.to_string()))).set_redirect_uri(redirect))
}

/// A claim the library doesn't model, from an ID token whose signature it already checked.
fn payload_claim(jwt: &str, name: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get(name)?.as_str().map(str::to_string)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An error and its causes, on one line.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    out
}
