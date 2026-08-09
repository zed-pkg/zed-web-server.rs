use sea_orm::DatabaseConnection;

pub struct WebState {
    /// None = "registry offline" mode: public package pages render with an
    /// empty-state banner instead of crashing.
    pub db: Option<DatabaseConnection>,
    pub registry_url: String,
    /// Internal base URL for the authenticated registry account API.
    pub api_url: String,
    /// Browser-visible product origin used for fail-closed form Origin checks.
    pub public_origin: String,
    /// First-party Shared Auth access-token cookie.
    pub session_cookie_name: String,
    /// Upstream base URL for the /shared-auth gateway path, trailing slashes
    /// trimmed. None = gateway disabled: those routes answer 503.
    pub shared_auth_url: Option<String>,
    /// Process-wide HTTP client for the gateway and account API. Redirects are
    /// deliberately disabled so neither boundary silently follows an upstream
    /// redirect with user credentials attached.
    pub http: reqwest::Client,
}
