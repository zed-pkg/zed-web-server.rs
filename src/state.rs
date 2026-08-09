use zed_orm_core::ReadContext;

pub struct WebState {
    /// None = "registry offline" mode: every page renders with an empty-state
    /// banner instead of crashing.
    ///
    /// A [`ReadContext`] rather than a raw connection: the type itself is the
    /// guarantee that no page can issue a write.
    pub db: Option<ReadContext>,
    /// Base URL of the API server. Every form on the site posts here.
    pub registry_url: String,
    /// Upstream base URL for the /shared-auth gateway path, trailing slashes
    /// trimmed. None = gateway disabled: those routes answer 503, and every
    /// viewer resolves as anonymous.
    pub shared_auth_url: Option<String>,
    /// Path on the shared-auth service that turns the sealed session cookie
    /// into claims. Configurable because the session cookie is sealed with a
    /// secret this process deliberately does not hold.
    pub session_path: String,
    /// Process-wide HTTP client for the gateway (redirect policy: none).
    pub http: reqwest::Client,
}
