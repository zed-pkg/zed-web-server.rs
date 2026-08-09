use zed_orm_core::ReadContext;

/// Browser-to-Shared-Auth trust boundary for app.zpkg.net.
///
/// Secret fields are intentionally not `Debug`: the handoff client secret and
/// cookie-signing key must never reach logs or diagnostics.
#[derive(Clone)]
pub struct BrowserAuthConfig {
    /// Cluster-internal Shared Auth origin.
    pub shared_auth_url: String,
    /// Browser-visible Shared Auth origin used for the authorize redirect.
    pub shared_auth_public_url: String,
    /// Exact public origin of this web application, without a trailing slash.
    pub public_origin: String,
    /// Cluster-internal zed API origin.
    pub api_url: String,
    /// Exact client registered for the one-time PKCE handoff.
    pub handoff_client_id: String,
    pub handoff_client_secret: String,
    /// OAuth authorized party (`azp`) used for API delegation.
    pub delegate_client_id: String,
    pub audience: String,
    pub scopes: Vec<String>,
    /// HMAC key for the host-only product-session cookie. Shared Auth remains
    /// the durable session owner; the cookie carries only its opaque refresh
    /// handle plus the canonical principal id.
    pub session_signing_secret: String,
    pub session_cookie_name: String,
    pub login_cookie_name: String,
    pub secure_cookies: bool,
}

pub struct WebState {
    /// None = "registry offline" mode: every page renders with an empty-state
    /// banner instead of crashing.
    ///
    /// A [`ReadContext`] rather than a raw connection: the type itself is the
    /// guarantee that no page can issue a write.
    pub db: Option<ReadContext>,
    /// Prefix retained for existing Maud form builders. The production server
    /// sets this to an empty string so every browser mutation is same-origin
    /// and handled by the BFF routes rather than posted cross-origin to the API.
    pub registry_url: String,
    /// Upstream base URL for the read-only `/shared-auth` gateway path.
    pub shared_auth_url: Option<String>,
    /// Legacy resolver path retained only for source-compatible test fixtures;
    /// product sessions now use the exact-client PKCE/BFF flow.
    #[allow(dead_code)]
    pub session_path: String,
    /// Exact-client PKCE, session, and delegated-token configuration.
    pub browser_auth: Option<BrowserAuthConfig>,
    /// Process-wide HTTP client for Shared Auth, API, and gateway calls.
    pub http: reqwest::Client,
}
