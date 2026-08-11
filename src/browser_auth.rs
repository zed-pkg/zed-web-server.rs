//! Browser authentication and same-origin mutation facade.
//!
//! Shared Auth owns the durable session in its RDS data plane. This web tier
//! keeps only the rotating opaque refresh handle and canonical principal id in
//! an HttpOnly, host-only, HMAC-protected cookie. Browser forms never receive a
//! registry bearer token: the backend refreshes, delegates, and forwards each
//! mutation to the Axum API over the cluster network.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::{BrowserAuthConfig, WebState};

const LOGIN_STATE_MAX_AGE_SECONDS: i64 = 10 * 60;
const SESSION_MAX_AGE_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserSession {
    refresh_token: String,
    shared_user_id: Uuid,
    issued_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoginState {
    state: String,
    verifier: String,
    return_to: String,
    issued_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct SignInQuery {
    #[serde(default = "default_return_to")]
    return_to: String,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct RedeemResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct ExchangeResponse {
    access_token: String,
    refresh_token: Option<String>,
    shared_user_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    shared_user_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct DelegateResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateOrgForm {
    slug: String,
    name: String,
}

#[derive(Debug, Deserialize)]
pub struct InvitationForm {
    email: String,
    role: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectForm {
    slug: String,
    name: String,
    #[allow(dead_code)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePackageForm {
    project_id: Option<String>,
    name: String,
    description: Option<String>,
    repo_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PackageSettingsForm {
    description: Option<String>,
    project_id: Option<String>,
    #[allow(dead_code)]
    repo_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VisibilityForm {
    visibility: String,
}

#[derive(Debug, Deserialize)]
pub struct UserSettingsForm {
    display_name: Option<String>,
    avatar_url: Option<String>,
}

fn default_return_to() -> String {
    "/".to_owned()
}

fn auth_config(state: &WebState) -> Option<&BrowserAuthConfig> {
    state.browser_auth.as_ref()
}

fn auth_unavailable() -> Response {
    error_json(
        StatusCode::SERVICE_UNAVAILABLE,
        "browser authentication is not configured",
    )
}

pub fn session_subject(state: &WebState, headers: &HeaderMap) -> Option<Uuid> {
    let config = state.browser_auth.as_ref()?;
    let session =
        read_signed_cookie::<BrowserSession>(headers, &config.session_cookie_name, config)?;
    let age = Utc::now().timestamp() - session.issued_at;
    (0..=SESSION_MAX_AGE_SECONDS)
        .contains(&age)
        .then_some(session.shared_user_id)
}

/// The result of an authenticated API GET. The generated bearer token remains
/// entirely inside this module; callers can only inspect the upstream result
/// and apply the rotated opaque browser session.
pub(crate) enum DelegatedGetOutcome {
    Upstream(reqwest::Response),
    Failed(Response),
}

pub(crate) struct RotatedSession(String);

impl RotatedSession {
    pub(crate) fn apply(self, response: &mut Response) {
        append_cookie(response, self.0);
    }
}

pub(crate) struct DelegatedGet {
    outcome: DelegatedGetOutcome,
    rotation: RotatedSession,
}

impl DelegatedGet {
    pub(crate) fn into_parts(self) -> (DelegatedGetOutcome, RotatedSession) {
        (self.outcome, self.rotation)
    }
}

pub(crate) async fn delegated_get(
    state: &WebState,
    headers: &HeaderMap,
    url: reqwest::Url,
    if_none_match: Option<HeaderValue>,
) -> Result<DelegatedGet, Response> {
    let Some(config) = auth_config(state) else {
        return Err(auth_unavailable());
    };
    if !delegated_url_allowed(config, &url) {
        tracing::error!(%url, "refused to send delegated credentials outside the configured API");
        return Err(error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid delegated API destination",
        ));
    }
    let Some(session) =
        read_signed_cookie::<BrowserSession>(headers, &config.session_cookie_name, config)
    else {
        return Err(error_json(
            StatusCode::UNAUTHORIZED,
            "browser session is unavailable",
        ));
    };
    let refreshed = refresh(state, config, &session).await?;
    let next_session = BrowserSession {
        refresh_token: refreshed.refresh_token,
        shared_user_id: refreshed.shared_user_id,
        issued_at: Utc::now().timestamp(),
    };
    let session_cookie = signed_cookie(
        &config.session_cookie_name,
        &next_session,
        SESSION_MAX_AGE_SECONDS,
        config,
    );
    let rotation = RotatedSession(session_cookie);
    let delegated = match delegate(state, config, &refreshed.access_token).await {
        Ok(token) => token,
        Err(response) => {
            return Ok(DelegatedGet {
                outcome: DelegatedGetOutcome::Failed(response),
                rotation,
            });
        }
    };
    let mut request = state.http.get(url).bearer_auth(delegated);
    if let Some(etag) = if_none_match {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    let outcome = match request.send().await {
        Ok(response) => DelegatedGetOutcome::Upstream(response),
        Err(error) => {
            tracing::warn!(%error, "delegated registry API read failed");
            DelegatedGetOutcome::Failed(error_json(
                StatusCode::BAD_GATEWAY,
                "registry API unavailable",
            ))
        }
    };
    Ok(DelegatedGet { outcome, rotation })
}

fn delegated_url_allowed(config: &BrowserAuthConfig, candidate: &reqwest::Url) -> bool {
    let Ok(base) = reqwest::Url::parse(&format!("{}/", config.api_url.trim_end_matches('/')))
    else {
        return false;
    };
    if candidate.origin() != base.origin()
        || !candidate.username().is_empty()
        || candidate.password().is_some()
        || candidate.fragment().is_some()
    {
        return false;
    }
    let base_path = base.path().trim_end_matches('/');
    base_path.is_empty()
        || candidate.path() == base_path
        || candidate
            .path()
            .strip_prefix(base_path)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub async fn sign_in(
    State(state): State<Arc<WebState>>,
    Query(query): Query<SignInQuery>,
) -> Response {
    let Some(config) = auth_config(&state) else {
        return auth_unavailable();
    };
    let return_to = sanitize_return_to(&query.return_to);
    let login = LoginState {
        state: random_token(),
        verifier: random_token(),
        return_to,
        issued_at: Utc::now().timestamp(),
    };
    let challenge = base64url_encode(&sha256(login.verifier.as_bytes()));
    let redirect_uri = callback_uri(config);
    let mut authorize =
        match reqwest::Url::parse(&format!("{}/authorize", config.shared_auth_public_url)) {
            Ok(url) => url,
            Err(error) => {
                tracing::error!(%error, "invalid Shared Auth public URL");
                return error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "invalid auth configuration",
                );
            }
        };
    authorize
        .query_pairs_mut()
        .append_pair("client_id", &config.handoff_client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("return_to", &login.return_to)
        .append_pair("state", &login.state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");

    let cookie = signed_cookie(
        &config.login_cookie_name,
        &login,
        LOGIN_STATE_MAX_AGE_SECONDS,
        config,
    );
    let mut response = Redirect::to(authorize.as_str()).into_response();
    append_cookie(&mut response, cookie);
    response
}

pub async fn callback(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(config) = auth_config(&state) else {
        return auth_unavailable();
    };
    let Some(login) = read_signed_cookie::<LoginState>(&headers, &config.login_cookie_name, config)
    else {
        return error_json(
            StatusCode::BAD_REQUEST,
            "login state cookie is missing or invalid",
        );
    };
    let age = Utc::now().timestamp() - login.issued_at;
    if !(0..=LOGIN_STATE_MAX_AGE_SECONDS).contains(&age)
        || !constant_time_eq(login.state.as_bytes(), query.state.as_bytes())
    {
        return error_json(StatusCode::BAD_REQUEST, "login state expired or mismatched");
    }

    let redeem = state
        .http
        .post(format!("{}/auth/handoff/redeem", config.shared_auth_url))
        .bearer_auth(&config.handoff_client_secret)
        .json(&json!({
            "client_id": config.handoff_client_id,
            "code": query.code,
            "redirect_uri": callback_uri(config),
            "code_verifier": login.verifier,
        }))
        .send()
        .await;
    let redeem = match redeem {
        Ok(response) => match decode_json::<RedeemResponse>(response, "handoff redemption").await {
            Ok(value) => value,
            Err(response) => return response,
        },
        Err(error) => {
            tracing::warn!(%error, "Shared Auth handoff redemption failed");
            return error_json(
                StatusCode::BAD_GATEWAY,
                "authentication upstream unavailable",
            );
        }
    };

    let exchange = state
        .http
        .post(format!("{}/auth/exchange", config.shared_auth_url))
        .bearer_auth(&redeem.access_token)
        .send()
        .await;
    let exchange = match exchange {
        Ok(response) => match decode_json::<ExchangeResponse>(response, "session exchange").await {
            Ok(value) => value,
            Err(response) => return response,
        },
        Err(error) => {
            tracing::warn!(%error, "Shared Auth exchange failed");
            return error_json(
                StatusCode::BAD_GATEWAY,
                "authentication upstream unavailable",
            );
        }
    };
    let Some(refresh_token) = exchange.refresh_token else {
        return error_json(
            StatusCode::BAD_GATEWAY,
            "Shared Auth did not issue a refresh handle",
        );
    };

    let delegated = match delegate(&state, config, &exchange.access_token).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    let projection = state
        .http
        .get(format!("{}/api/v1/account/me", config.api_url))
        .bearer_auth(&delegated)
        .send()
        .await;
    match projection {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            tracing::warn!(status = %response.status(), "registry user projection failed");
            return error_json(
                StatusCode::BAD_GATEWAY,
                "registry account initialization failed",
            );
        }
        Err(error) => {
            tracing::warn!(%error, "registry API unavailable during login");
            return error_json(StatusCode::BAD_GATEWAY, "registry API unavailable");
        }
    }

    let session = BrowserSession {
        refresh_token,
        shared_user_id: exchange.shared_user_id,
        issued_at: Utc::now().timestamp(),
    };
    let session_cookie = signed_cookie(
        &config.session_cookie_name,
        &session,
        SESSION_MAX_AGE_SECONDS,
        config,
    );
    let mut response = Redirect::to(&sanitize_return_to(&login.return_to)).into_response();
    append_cookie(&mut response, session_cookie);
    append_cookie(
        &mut response,
        clear_cookie(&config.login_cookie_name, config),
    );
    response
}

pub async fn logout(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(config) = auth_config(&state) else {
        return auth_unavailable();
    };
    if !same_origin_request(&headers, config) {
        return error_json(StatusCode::FORBIDDEN, "cross-origin logout rejected");
    }
    if let Some(session) =
        read_signed_cookie::<BrowserSession>(&headers, &config.session_cookie_name, config)
    {
        let result = state
            .http
            .post(format!("{}/auth/logout", config.shared_auth_url))
            .json(&json!({ "refresh_token": session.refresh_token }))
            .send()
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "Shared Auth logout request failed");
        }
    }
    let mut response = Redirect::to("/").into_response();
    append_cookie(
        &mut response,
        clear_cookie(&config.session_cookie_name, config),
    );
    response
}

pub async fn create_org(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<CreateOrgForm>,
) -> Response {
    mutate(
        &state,
        &headers,
        Method::POST,
        "/api/v1/account/orgs".to_owned(),
        json!({
            "slug": form.slug,
            "name": form.name,
            "description": null,
            "settings": {},
        }),
    )
    .await
}

pub async fn invite_org(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Form(form): Form<InvitationForm>,
) -> Response {
    mutate(
        &state,
        &headers,
        Method::POST,
        format!("/api/v1/account/orgs/{org}/invitations"),
        json!({ "email": form.email, "role": form.role }),
    )
    .await
}

pub async fn create_project(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Form(form): Form<CreateProjectForm>,
) -> Response {
    mutate(
        &state,
        &headers,
        Method::POST,
        format!("/api/v1/account/orgs/{org}/projects"),
        json!({ "slug": form.slug, "name": form.name }),
    )
    .await
}

pub async fn create_package(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Form(form): Form<CreatePackageForm>,
) -> Response {
    mutate(
        &state,
        &headers,
        Method::POST,
        format!("/api/v1/account/orgs/{org}/packages"),
        json!({
            "project_id": parse_optional_uuid(form.project_id),
            "name": form.name,
            "description": form.description,
            "vcs": "git",
            "repo_url": form.repo_url.unwrap_or_default(),
            "config": {},
            "default_archive_format": "tar.gz",
        }),
    )
    .await
}

pub async fn invite_project(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(project_id): Path<Uuid>,
    Form(form): Form<InvitationForm>,
) -> Response {
    let viewer = crate::session::resolve(&state, &headers).await;
    let Some(db) = &state.db else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "registry database unavailable",
        );
    };
    let mut target = None;
    for org in viewer.orgs() {
        let projects = zed_orm_core::read::projects_for_org(db, org.id, &org.slug, true)
            .await
            .unwrap_or_default();
        if let Some(project) = projects
            .into_iter()
            .find(|project| project.id == project_id)
        {
            target = Some((org.slug.clone(), project.slug));
            break;
        }
    }
    let Some((org, project)) = target else {
        return error_json(StatusCode::NOT_FOUND, "project not found");
    };
    mutate(
        &state,
        &headers,
        Method::POST,
        format!("/api/v1/account/orgs/{org}/projects/{project}/invitations"),
        json!({ "email": form.email, "role": form.role }),
    )
    .await
}

pub async fn update_package(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, package)): Path<(String, String)>,
    Form(form): Form<PackageSettingsForm>,
) -> Response {
    mutate(
        &state,
        &headers,
        Method::PUT,
        format!("/api/v1/account/orgs/{org}/packages/{package}/settings"),
        json!({
            "description": form.description,
            "project_id": parse_optional_uuid(form.project_id),
            "config": {},
        }),
    )
    .await
}

pub async fn make_public(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, package)): Path<(String, String)>,
    Form(form): Form<VisibilityForm>,
) -> Response {
    if form.visibility != "public" {
        return error_json(
            StatusCode::BAD_REQUEST,
            "only promotion to public is supported",
        );
    }
    mutate(
        &state,
        &headers,
        Method::POST,
        format!("/api/v1/account/orgs/{org}/packages/{package}/public"),
        json!({}),
    )
    .await
}

pub async fn update_user(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<UserSettingsForm>,
) -> Response {
    mutate(
        &state,
        &headers,
        Method::PUT,
        "/api/v1/account/me".to_owned(),
        json!({
            "display_name": form.display_name,
            "avatar_url": form.avatar_url,
            "settings": {},
        }),
    )
    .await
}

async fn mutate(
    state: &WebState,
    headers: &HeaderMap,
    method: Method,
    path: String,
    body: Value,
) -> Response {
    let Some(config) = auth_config(state) else {
        return auth_unavailable();
    };
    if !same_origin_request(headers, config) {
        return error_json(StatusCode::FORBIDDEN, "cross-origin mutation rejected");
    }
    let Some(session) =
        read_signed_cookie::<BrowserSession>(headers, &config.session_cookie_name, config)
    else {
        return sign_in_redirect(headers);
    };

    let refreshed = match refresh(state, config, &session).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let delegated = match delegate(state, config, &refreshed.access_token).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    let api = state
        .http
        .request(method, format!("{}{}", config.api_url, path))
        .bearer_auth(delegated)
        .json(&body)
        .send()
        .await;

    let next_session = BrowserSession {
        refresh_token: refreshed.refresh_token,
        shared_user_id: refreshed.shared_user_id,
        issued_at: Utc::now().timestamp(),
    };
    let cookie = signed_cookie(
        &config.session_cookie_name,
        &next_session,
        SESSION_MAX_AGE_SECONDS,
        config,
    );

    match api {
        Ok(response) if response.status().is_success() => {
            let mut response = Redirect::to(&redirect_target(headers, config)).into_response();
            append_cookie(&mut response, cookie);
            response
        }
        Ok(response) => {
            let status = response.status();
            let bytes = response.bytes().await.unwrap_or_default();
            let mut downstream = Response::new(Body::from(bytes));
            *downstream.status_mut() = status;
            downstream.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            append_cookie(&mut downstream, cookie);
            downstream
        }
        Err(error) => {
            tracing::warn!(%error, "registry API mutation failed");
            let mut response = error_json(StatusCode::BAD_GATEWAY, "registry API unavailable");
            append_cookie(&mut response, cookie);
            response
        }
    }
}

fn sign_in_redirect(headers: &HeaderMap) -> Response {
    let return_to = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| reqwest::Url::parse(value).ok())
        .map(|url| match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        })
        .unwrap_or_else(default_return_to);
    Redirect::to(&format!(
        "/auth/sign-in?return_to={}",
        percent_encode(&return_to)
    ))
    .into_response()
}

async fn refresh(
    state: &WebState,
    config: &BrowserAuthConfig,
    session: &BrowserSession,
) -> Result<RefreshResponse, Response> {
    let response = state
        .http
        .post(format!("{}/auth/refresh", config.shared_auth_url))
        .json(&json!({ "refresh_token": session.refresh_token }))
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Shared Auth refresh failed");
            error_json(
                StatusCode::BAD_GATEWAY,
                "authentication upstream unavailable",
            )
        })?;
    let refreshed = decode_json::<RefreshResponse>(response, "session refresh").await?;
    if refreshed.shared_user_id != session.shared_user_id {
        return Err(error_json(
            StatusCode::UNAUTHORIZED,
            "refreshed session principal changed",
        ));
    }
    Ok(refreshed)
}

async fn delegate(
    state: &WebState,
    config: &BrowserAuthConfig,
    base_access_token: &str,
) -> Result<String, Response> {
    let response = state
        .http
        .post(format!("{}/auth/delegate", config.shared_auth_url))
        .bearer_auth(base_access_token)
        .json(&json!({
            "client_id": &config.delegate_client_id,
            "audience": &config.audience,
            "scopes": &config.scopes,
        }))
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Shared Auth delegation failed");
            error_json(
                StatusCode::BAD_GATEWAY,
                "authentication upstream unavailable",
            )
        })?;
    Ok(
        decode_json::<DelegateResponse>(response, "token delegation")
            .await?
            .access_token,
    )
}

async fn decode_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    operation: &str,
) -> Result<T, Response> {
    let status = response.status();
    if !status.is_success() {
        let response_bytes = response.content_length();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("<missing>");
        tracing::warn!(
            %status,
            %operation,
            ?response_bytes,
            %request_id,
            "upstream authentication operation failed"
        );
        let mapped = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            StatusCode::UNAUTHORIZED
        } else if status.is_client_error() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::BAD_GATEWAY
        };
        return Err(error_json(mapped, "authentication operation failed"));
    }
    response.json::<T>().await.map_err(|error| {
        tracing::warn!(%error, %operation, "authentication response was invalid");
        error_json(
            StatusCode::BAD_GATEWAY,
            "authentication response was invalid",
        )
    })
}

fn same_origin_request(headers: &HeaderMap, config: &BrowserAuthConfig) -> bool {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return origin == config.public_origin;
    }
    headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| reqwest::Url::parse(value).ok())
        .map(|url| url.origin().ascii_serialization() == config.public_origin)
        .unwrap_or(false)
}

fn redirect_target(headers: &HeaderMap, config: &BrowserAuthConfig) -> String {
    headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| reqwest::Url::parse(value).ok())
        .filter(|url| url.origin().ascii_serialization() == config.public_origin)
        .map(|url| match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        })
        .map(|value| sanitize_return_to(&value))
        .unwrap_or_else(default_return_to)
}

fn callback_uri(config: &BrowserAuthConfig) -> String {
    format!("{}/auth/shared/callback", config.public_origin)
}

fn sanitize_return_to(value: &str) -> String {
    if return_target_is_local(value) {
        value.to_owned()
    } else {
        default_return_to()
    }
}

fn return_target_is_local(value: &str) -> bool {
    let mut candidate = value.as_bytes().to_vec();
    for _ in 0..=8 {
        let Ok(text) = std::str::from_utf8(&candidate) else {
            return false;
        };
        if !text.starts_with('/')
            || text.starts_with("//")
            || text.starts_with("/\\")
            || text
                .chars()
                .any(|character| character == '\\' || character == '#' || character.is_control())
        {
            return false;
        }

        let (decoded, changed) = percent_decode_once(text);
        if !changed {
            return true;
        }
        candidate = decoded;
    }
    false
}

fn percent_decode_once(value: &str) -> (Vec<u8>, bool) {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut changed = false;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_nibble(bytes[index + 1]), hex_nibble(bytes[index + 2]))
        {
            output.push((high << 4) | low);
            index += 3;
            changed = true;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    (output, changed)
}

fn random_token() -> String {
    format!(
        "{}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn signed_cookie<T: Serialize>(
    name: &str,
    value: &T,
    max_age: i64,
    config: &BrowserAuthConfig,
) -> String {
    let json = serde_json::to_vec(value).expect("session structs are JSON serializable");
    let payload = hex_encode(&json);
    let signature = hex_encode(&hmac_sha256(
        config.session_signing_secret.as_bytes(),
        payload.as_bytes(),
    ));
    format!(
        "{name}={payload}.{signature}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if config.secure_cookies {
            "; Secure"
        } else {
            ""
        }
    )
}

fn clear_cookie(name: &str, config: &BrowserAuthConfig) -> String {
    format!(
        "{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if config.secure_cookies {
            "; Secure"
        } else {
            ""
        }
    )
}

fn read_signed_cookie<T: for<'de> Deserialize<'de>>(
    headers: &HeaderMap,
    name: &str,
    config: &BrowserAuthConfig,
) -> Option<T> {
    let value = cookie_value(headers, name)?;
    let (payload, signature) = value.split_once('.')?;
    let expected = hex_encode(&hmac_sha256(
        config.session_signing_secret.as_bytes(),
        payload.as_bytes(),
    ));
    if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        return None;
    }
    let decoded = hex_decode(payload)?;
    serde_json::from_slice(&decoded).ok()
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn append_cookie(response: &mut Response, cookie: String) {
    match HeaderValue::from_str(&cookie) {
        Ok(value) => {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
        Err(error) => tracing::error!(%error, "refused to emit malformed session cookie"),
    }
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn percent_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn base64url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        output.push(char::from(TABLE[usize::from(first >> 2)]));
        output.push(char::from(
            TABLE[usize::from(((first & 0x03) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                TABLE[usize::from(((second & 0x0f) << 2) | (third >> 6))],
            ));
        }
        if chunk.len() > 2 {
            output.push(char::from(TABLE[usize::from(third & 0x3f)]));
        }
    }
    output
}

fn parse_optional_uuid(value: Option<String>) -> Option<Uuid> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse().ok())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut normalized = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        normalized[..32].copy_from_slice(&sha256(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = [0x36_u8; BLOCK_SIZE];
    let mut outer_key = [0x5c_u8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        inner_key[index] ^= normalized[index];
        outer_key[index] ^= normalized[index];
    }
    let mut inner = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner.extend_from_slice(&inner_key);
    inner.extend_from_slice(message);
    let inner_hash = sha256(&inner);
    let mut outer = Vec::with_capacity(BLOCK_SIZE + inner_hash.len());
    outer.extend_from_slice(&outer_key);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temporary1 = h
                .wrapping_add(sum1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary1);
            d = c;
            c = b;
            b = a;
            a = temporary1.wrapping_add(temporary2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut output = [0_u8; 32];
    for (index, word) in state.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BrowserAuthConfig {
        BrowserAuthConfig {
            shared_auth_url: "http://auth.internal".into(),
            shared_auth_public_url: "https://auth.example.test".into(),
            public_origin: "https://app.zpkg.net".into(),
            api_url: "http://api.internal".into(),
            handoff_client_id: "zpkg".into(),
            handoff_client_secret: "handoff-secret".into(),
            delegate_client_id: "zpkg-web".into(),
            audience: "zed-pkg".into(),
            scopes: vec!["zpkg:account".into()],
            session_signing_secret: "0123456789abcdef0123456789abcdef".into(),
            session_cookie_name: "__Host-zpkg_session".into(),
            login_cookie_name: "__Host-zpkg_login".into(),
            secure_cookies: true,
        }
    }

    #[test]
    fn sha256_matches_the_standard_abc_vector() {
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_matches_the_rfc_4231_case_one_vector() {
        assert_eq!(
            hex_encode(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn base64url_is_unpadded() {
        assert_eq!(base64url_encode(b"f"), "Zg");
        assert_eq!(base64url_encode(b"fo"), "Zm8");
        assert_eq!(base64url_encode(b"foo"), "Zm9v");
    }

    #[test]
    fn signed_cookie_round_trips_and_rejects_tampering() {
        let config = config();
        let session = BrowserSession {
            refresh_token: "refresh".into(),
            shared_user_id: Uuid::nil(),
            issued_at: 123,
        };
        let cookie = signed_cookie("test", &session, 60, &config);
        let pair = cookie.split(';').next().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, pair.parse().unwrap());
        let decoded = read_signed_cookie::<BrowserSession>(&headers, "test", &config).unwrap();
        assert_eq!(decoded.refresh_token, "refresh");

        let mut tampered = pair.to_owned();
        tampered.push('0');
        headers.insert(header::COOKIE, tampered.parse().unwrap());
        assert!(read_signed_cookie::<BrowserSession>(&headers, "test", &config).is_none());
    }

    #[test]
    fn return_targets_are_local_paths_only() {
        for valid in [
            "/",
            "/dashboard/acme?tab=1",
            "/search?q=hello%20world",
            "/search?next=%2Fpackages",
        ] {
            assert_eq!(sanitize_return_to(valid), valid);
        }
        for invalid in [
            "https://evil.test",
            "//evil.test",
            "/\\evil.test",
            "/ok#fragment",
            "/ok\r\nLocation: bad",
            "/ok\tbad",
            "/%2Fevil.test",
            "/%5Cevil.test",
            "/%255cevil.test",
            "/ok%23fragment",
            "/ok%0Abad",
            "/ok%250Abad",
            "/ok%00bad",
            "/ok%7Fbad",
        ] {
            assert_eq!(sanitize_return_to(invalid), "/");
        }
    }

    #[test]
    fn delegated_reads_stay_on_the_configured_api_origin_and_base_path() {
        let mut config = config();
        config.api_url = "https://api.internal/base".into();
        assert!(delegated_url_allowed(
            &config,
            &"https://api.internal/base/v1/packages/acme/http"
                .parse()
                .unwrap()
        ));
        assert!(!delegated_url_allowed(
            &config,
            &"https://api.internal/baseball/v1/packages/acme/http"
                .parse()
                .unwrap()
        ));
        assert!(!delegated_url_allowed(
            &config,
            &"https://evil.internal/base/v1/packages/acme/http"
                .parse()
                .unwrap()
        ));
    }

    #[test]
    fn rotated_private_read_session_is_applied_to_every_downstream_status() {
        let mut response = StatusCode::BAD_GATEWAY.into_response();
        RotatedSession("session=rotated; Path=/; HttpOnly".into()).apply(&mut response);
        assert_eq!(
            response.headers()[header::SET_COOKIE],
            "session=rotated; Path=/; HttpOnly"
        );
    }

    #[test]
    fn authentication_failures_never_log_upstream_response_bodies() {
        let source = include_str!("browser_auth.rs");
        let body_log = ["body", " = %body"].concat();
        let body_read = ["response", ".text().await"].concat();
        assert!(!source.contains(&body_log));
        assert!(!source.contains(&body_read));
    }
}
