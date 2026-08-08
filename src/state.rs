use sea_orm::DatabaseConnection;

pub struct WebState {
    /// None = "registry offline" mode: every page renders with an empty
    /// state banner instead of crashing.
    pub db: Option<DatabaseConnection>,
    pub registry_url: String,
    /// Upstream base URL for the /shared-auth gateway path, trailing slashes
    /// trimmed. None = gateway disabled: those routes answer 503.
    pub shared_auth_url: Option<String>,
    /// Process-wide HTTP client for the gateway (redirect policy: none).
    pub http: reqwest::Client,
}
