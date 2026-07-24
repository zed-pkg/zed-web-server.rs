use sea_orm::DatabaseConnection;

pub struct WebState {
    /// None = "registry offline" mode: every page renders with an empty
    /// state banner instead of crashing.
    pub db: Option<DatabaseConnection>,
    pub registry_url: String,
}
