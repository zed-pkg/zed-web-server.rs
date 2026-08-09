use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use maud::{DOCTYPE, Markup, html};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::WebState;

const ACCOUNT_BODY_LIMIT: usize = 64 * 1024;
const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(10);
const ACCOUNT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
     object-src 'none'";

#[derive(Clone, Debug, Deserialize)]
struct UserResponse {
    subject: String,
    email: Option<String>,
    display_name: Option<String>,
    avatar_url: Option<String>,
    settings: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct OrgResponse {
    slug: String,
    name: String,
    description: Option<String>,
    role: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ProjectResponse {
    id: String,
    org_slug: String,
    slug: String,
    name: String,
    description: Option<String>,
    role: String,
}

#[derive(Clone, Debug, Deserialize)]
struct PackageResponse {
    org_slug: String,
    project_id: Option<String>,
    name: String,
    description: Option<String>,
    visibility: String,
    repo_url: String,
    config: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct HomeResponse {
    user: Option<UserResponse>,
    orgs: Vec<OrgResponse>,
    projects: Vec<ProjectResponse>,
    packages: Vec<PackageResponse>,
    query: String,
}

#[derive(Clone, Debug, Deserialize)]
struct DashboardResponse {
    org: OrgResponse,
    projects: Vec<ProjectResponse>,
    packages: Vec<PackageResponse>,
}

#[derive(Clone, Debug, Deserialize)]
struct InvitationResponse {
    invitation_id: String,
    email: String,
    role: String,
    token: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct HomeQuery {
    #[serde(default)]
    q: String,
}

#[derive(Debug, Deserialize, Default)]
struct ReturnQuery {
    return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateOrgForm {
    slug: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct CreateProjectForm {
    slug: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct InviteForm {
    email: String,
    role: String,
}

#[derive(Debug, Deserialize)]
struct PackageForm {
    description: String,
    visibility: String,
    project_id: String,
    config: String,
}

#[derive(Debug, Deserialize)]
struct UserForm {
    display_name: String,
    avatar_url: String,
    settings: String,
}

#[derive(Debug, Deserialize)]
struct AcceptQuery {
    token: String,
}

#[derive(Debug, Deserialize)]
struct AcceptForm {
    token: String,
}

#[derive(Debug, Serialize)]
struct CreateOrgRequest<'a> {
    slug: &'a str,
    name: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateProjectRequest<'a> {
    slug: &'a str,
    name: &'a str,
}

#[derive(Debug, Serialize)]
struct InviteRequest<'a> {
    email: &'a str,
    role: &'a str,
}

#[derive(Debug, Serialize)]
struct PackageRequest {
    description: Option<String>,
    project_id: Option<String>,
    visibility: String,
    config: Value,
}

#[derive(Debug, Serialize)]
struct UserRequest {
    display_name: Option<String>,
    avatar_url: Option<String>,
    settings: Value,
}

#[derive(Debug, Serialize)]
struct AcceptRequest<'a> {
    token: &'a str,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: Option<String>,
}

#[derive(Debug)]
enum ApiFailure {
    Unauthorized,
    NotFound,
    Rejected { status: u16, message: String },
    Unavailable(String),
}

pub fn router(state: Arc<WebState>) -> Router {
    Router::new()
        .route("/login", get(login))
        .route("/signup", get(signup))
        .route("/dashboard", get(dashboard).post(create_organization))
        .route(
            "/orgs/{org}/dashboard",
            get(org_dashboard).post(create_project),
        )
        .route(
            "/orgs/{org}/settings",
            get(org_settings).post(invite_org_member),
        )
        .route(
            "/orgs/{org}/projects/{project}/settings",
            get(project_settings).post(invite_project_member),
        )
        .route(
            "/orgs/{org}/packages/{package}/settings",
            get(package_settings).post(update_package_settings),
        )
        .route("/settings", get(user_settings).post(update_user_settings))
        .route(
            "/invitations/accept",
            get(accept_invitation_page).post(accept_invitation),
        )
        .layer(DefaultBodyLimit::max(ACCOUNT_BODY_LIMIT))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            ACCOUNT_TIMEOUT,
        ))
        .layer(security_header(
            header::CONTENT_SECURITY_POLICY,
            ACCOUNT_CSP,
        ))
        .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(security_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(security_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ))
        .with_state(state)
}

/// The authenticated home page replaces the public registry landing page when
/// a valid Shared Auth cookie is present. Invalid/expired sessions fall back to
/// the public package landing page rather than leaking account state.
pub(crate) async fn home(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(query): Query<HomeQuery>,
) -> Response {
    let Some(token) = session_token(&headers, &state) else {
        return crate::routes::public_home(State(state))
            .await
            .into_response();
    };
    let path = format!("/v1/me/home?q={}", query_component(&query.q));
    match api_request::<HomeResponse>(&state, &token, Method::GET, &path, None).await {
        Ok(home) => account_home_page(&state, home).into_response(),
        Err(ApiFailure::Unauthorized) => crate::routes::public_home(State(state))
            .await
            .into_response(),
        Err(error) => api_failure_page(&state, None, "Home unavailable", error),
    }
}

async fn login(Query(query): Query<ReturnQuery>) -> Redirect {
    Redirect::to(&shared_auth_sign_in(
        query.return_to.as_deref().unwrap_or("/dashboard"),
    ))
}

async fn signup(Query(query): Query<ReturnQuery>) -> Redirect {
    Redirect::to(&shared_auth_sign_in(
        query.return_to.as_deref().unwrap_or("/dashboard"),
    ))
}

async fn dashboard(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect("/dashboard");
    };
    match api_request::<HomeResponse>(&state, &token, Method::GET, "/v1/me/home", None).await {
        Ok(home) => dashboard_page(&state, home, None).into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect("/dashboard"),
        Err(error) => api_failure_page(&state, None, "Dashboard unavailable", error),
    }
}

async fn create_organization(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<CreateOrgForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect("/dashboard");
    };
    let body = json!(CreateOrgRequest {
        slug: form.slug.trim(),
        name: form.name.trim(),
    });
    match api_request::<OrgResponse>(&state, &token, Method::POST, "/v1/account/orgs", Some(body))
        .await
    {
        Ok(org) => {
            Redirect::to(&format!("/orgs/{}/dashboard", path_segment(&org.slug))).into_response()
        }
        Err(ApiFailure::Unauthorized) => login_redirect("/dashboard"),
        Err(error) => {
            match api_request::<HomeResponse>(&state, &token, Method::GET, "/v1/me/home", None)
                .await
            {
                Ok(home) => {
                    dashboard_page(&state, home, Some(failure_message(error))).into_response()
                }
                Err(second) => {
                    api_failure_page(&state, None, "Organization was not created", second)
                }
            }
        }
    }
}

async fn org_dashboard(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Response {
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&format!("/orgs/{}/dashboard", path_segment(&org)));
    };
    match fetch_dashboard(&state, &token, &org).await {
        Ok(dashboard) => org_dashboard_page(&state, dashboard, None).into_response(),
        Err(ApiFailure::Unauthorized) => {
            login_redirect(&format!("/orgs/{}/dashboard", path_segment(&org)))
        }
        Err(error) => api_failure_page(&state, None, "Organization unavailable", error),
    }
}

async fn create_project(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Form(form): Form<CreateProjectForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&format!("/orgs/{}/dashboard", path_segment(&org)));
    };
    let body = json!(CreateProjectRequest {
        slug: form.slug.trim(),
        name: form.name.trim(),
    });
    let path = format!("/v1/account/orgs/{}/projects", path_segment(&org));
    match api_request::<ProjectResponse>(&state, &token, Method::POST, &path, Some(body)).await {
        Ok(project) => Redirect::to(&format!(
            "/orgs/{}/projects/{}/settings",
            path_segment(&org),
            path_segment(&project.slug)
        ))
        .into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect("/dashboard"),
        Err(error) => match fetch_dashboard(&state, &token, &org).await {
            Ok(dashboard) => {
                org_dashboard_page(&state, dashboard, Some(failure_message(error))).into_response()
            }
            Err(second) => api_failure_page(&state, None, "Project was not created", second),
        },
    }
}

async fn org_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Response {
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&format!("/orgs/{}/settings", path_segment(&org)));
    };
    match fetch_dashboard(&state, &token, &org).await {
        Ok(dashboard) => org_settings_page(&state, dashboard, None, None).into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect("/dashboard"),
        Err(error) => api_failure_page(&state, None, "Organization settings unavailable", error),
    }
}

async fn invite_org_member(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Form(form): Form<InviteForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&format!("/orgs/{}/settings", path_segment(&org)));
    };
    let path = format!("/v1/account/orgs/{}/invitations", path_segment(&org));
    let body = json!(InviteRequest {
        email: form.email.trim(),
        role: form.role.trim(),
    });
    match api_request::<InvitationResponse>(&state, &token, Method::POST, &path, Some(body)).await {
        Ok(invitation) => match fetch_dashboard(&state, &token, &org).await {
            Ok(dashboard) => {
                org_settings_page(&state, dashboard, None, Some(invitation)).into_response()
            }
            Err(error) => api_failure_page(&state, None, "Invitation created", error),
        },
        Err(ApiFailure::Unauthorized) => login_redirect("/dashboard"),
        Err(error) => match fetch_dashboard(&state, &token, &org).await {
            Ok(dashboard) => {
                org_settings_page(&state, dashboard, Some(failure_message(error)), None)
                    .into_response()
            }
            Err(second) => api_failure_page(&state, None, "Invitation was not created", second),
        },
    }
}

async fn project_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, project)): Path<(String, String)>,
) -> Response {
    let return_to = format!(
        "/orgs/{}/projects/{}/settings",
        path_segment(&org),
        path_segment(&project)
    );
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&return_to);
    };
    match fetch_project(&state, &token, &org, &project).await {
        Ok(project) => project_settings_page(&state, project, None, None).into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect(&return_to),
        Err(error) => api_failure_page(&state, None, "Project settings unavailable", error),
    }
}

async fn invite_project_member(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, project)): Path<(String, String)>,
    Form(form): Form<InviteForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let return_to = format!(
        "/orgs/{}/projects/{}/settings",
        path_segment(&org),
        path_segment(&project)
    );
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&return_to);
    };
    let path = format!(
        "/v1/account/orgs/{}/projects/{}/invitations",
        path_segment(&org),
        path_segment(&project)
    );
    let body = json!(InviteRequest {
        email: form.email.trim(),
        role: form.role.trim(),
    });
    match api_request::<InvitationResponse>(&state, &token, Method::POST, &path, Some(body)).await {
        Ok(invitation) => match fetch_project(&state, &token, &org, &project).await {
            Ok(project) => {
                project_settings_page(&state, project, None, Some(invitation)).into_response()
            }
            Err(error) => api_failure_page(&state, None, "Invitation created", error),
        },
        Err(ApiFailure::Unauthorized) => login_redirect(&return_to),
        Err(error) => match fetch_project(&state, &token, &org, &project).await {
            Ok(project) => {
                project_settings_page(&state, project, Some(failure_message(error)), None)
                    .into_response()
            }
            Err(second) => api_failure_page(&state, None, "Invitation was not created", second),
        },
    }
}

async fn package_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, package)): Path<(String, String)>,
) -> Response {
    let return_to = format!(
        "/orgs/{}/packages/{}/settings",
        path_segment(&org),
        path_segment(&package)
    );
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&return_to);
    };
    match fetch_package_and_dashboard(&state, &token, &org, &package).await {
        Ok((package, dashboard)) => {
            package_settings_page(&state, package, dashboard.projects, None).into_response()
        }
        Err(ApiFailure::Unauthorized) => login_redirect(&return_to),
        Err(error) => api_failure_page(&state, None, "Package settings unavailable", error),
    }
}

async fn update_package_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, package)): Path<(String, String)>,
    Form(form): Form<PackageForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let return_to = format!(
        "/orgs/{}/packages/{}/settings",
        path_segment(&org),
        path_segment(&package)
    );
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&return_to);
    };
    let config = match parse_json_object(&form.config, "package configuration") {
        Ok(config) => config,
        Err(message) => {
            return match fetch_package_and_dashboard(&state, &token, &org, &package).await {
                Ok((package, dashboard)) => {
                    package_settings_page(&state, package, dashboard.projects, Some(message))
                        .into_response()
                }
                Err(error) => api_failure_page(&state, None, "Package settings unavailable", error),
            };
        }
    };
    let path = format!(
        "/v1/account/orgs/{}/packages/{}",
        path_segment(&org),
        path_segment(&package)
    );
    let body = json!(PackageRequest {
        description: optional_text(&form.description),
        project_id: optional_text(&form.project_id),
        visibility: form.visibility,
        config,
    });
    match api_request::<PackageResponse>(&state, &token, Method::PATCH, &path, Some(body)).await {
        Ok(_) => Redirect::to(&return_to).into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect(&return_to),
        Err(error) => match fetch_package_and_dashboard(&state, &token, &org, &package).await {
            Ok((package, dashboard)) => package_settings_page(
                &state,
                package,
                dashboard.projects,
                Some(failure_message(error)),
            )
            .into_response(),
            Err(second) => api_failure_page(&state, None, "Package was not updated", second),
        },
    }
}

async fn user_settings(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect("/settings");
    };
    match api_request::<UserResponse>(&state, &token, Method::GET, "/v1/me", None).await {
        Ok(user) => user_settings_page(&state, user, None).into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect("/settings"),
        Err(error) => api_failure_page(&state, None, "User settings unavailable", error),
    }
}

async fn update_user_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<UserForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect("/settings");
    };
    let settings = match parse_json_object(&form.settings, "user settings") {
        Ok(settings) => settings,
        Err(message) => {
            return match api_request::<UserResponse>(&state, &token, Method::GET, "/v1/me", None)
                .await
            {
                Ok(user) => user_settings_page(&state, user, Some(message)).into_response(),
                Err(error) => api_failure_page(&state, None, "User settings unavailable", error),
            };
        }
    };
    let body = json!(UserRequest {
        display_name: optional_text(&form.display_name),
        avatar_url: optional_text(&form.avatar_url),
        settings,
    });
    match api_request::<UserResponse>(&state, &token, Method::PATCH, "/v1/me", Some(body)).await {
        Ok(_) => Redirect::to("/settings").into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect("/settings"),
        Err(error) => {
            match api_request::<UserResponse>(&state, &token, Method::GET, "/v1/me", None).await {
                Ok(user) => {
                    user_settings_page(&state, user, Some(failure_message(error))).into_response()
                }
                Err(second) => {
                    api_failure_page(&state, None, "User settings were not updated", second)
                }
            }
        }
    }
}

async fn accept_invitation_page(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(query): Query<AcceptQuery>,
) -> Response {
    if query.token.len() < 32 || query.token.len() > 256 {
        return simple_page(
            &state,
            None,
            StatusCode::BAD_REQUEST,
            "Invalid invitation",
            "The invitation token is malformed.",
        );
    }
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&format!(
            "/invitations/accept?token={}",
            query_component(&query.token)
        ));
    };
    let user = api_request::<UserResponse>(&state, &token, Method::GET, "/v1/me", None)
        .await
        .ok();
    Html(
        account_layout(
            "Accept invitation",
            user.as_ref(),
            html! {
                section class="console-card narrow" {
                    p class="eyebrow" { "Invitation" }
                    h1 { "Join this registry workspace" }
                    p class="muted" {
                        "Acceptance is bound to your verified Shared Auth identity and email."
                    }
                    form method="post" action="/invitations/accept" class="stack" {
                        input type="hidden" name="token" value=(query.token);
                        button type="submit" class="button primary" { "Accept invitation" }
                    }
                }
            },
        )
        .into_string(),
    )
    .into_response()
}

async fn accept_invitation(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<AcceptForm>,
) -> Response {
    if let Err(response) = require_same_origin(&headers, &state) {
        return *response;
    }
    let return_to = format!("/invitations/accept?token={}", query_component(&form.token));
    let Some(token) = session_token(&headers, &state) else {
        return login_redirect(&return_to);
    };
    let body = json!(AcceptRequest { token: &form.token });
    match api_request::<Value>(
        &state,
        &token,
        Method::POST,
        "/v1/invitations/accept",
        Some(body),
    )
    .await
    {
        Ok(_) => Redirect::to("/dashboard").into_response(),
        Err(ApiFailure::Unauthorized) => login_redirect(&return_to),
        Err(error) => api_failure_page(&state, None, "Invitation was not accepted", error),
    }
}

async fn fetch_dashboard(
    state: &WebState,
    token: &str,
    org: &str,
) -> Result<DashboardResponse, ApiFailure> {
    api_request(
        state,
        token,
        Method::GET,
        &format!("/v1/account/orgs/{}/dashboard", path_segment(org)),
        None,
    )
    .await
}

async fn fetch_project(
    state: &WebState,
    token: &str,
    org: &str,
    project: &str,
) -> Result<ProjectResponse, ApiFailure> {
    api_request(
        state,
        token,
        Method::GET,
        &format!(
            "/v1/account/orgs/{}/projects/{}",
            path_segment(org),
            path_segment(project)
        ),
        None,
    )
    .await
}

async fn fetch_package_and_dashboard(
    state: &WebState,
    token: &str,
    org: &str,
    package: &str,
) -> Result<(PackageResponse, DashboardResponse), ApiFailure> {
    let package_path = format!(
        "/v1/account/orgs/{}/packages/{}",
        path_segment(org),
        path_segment(package)
    );
    let (package, dashboard) = tokio::join!(
        api_request(state, token, Method::GET, &package_path, None),
        fetch_dashboard(state, token, org),
    );
    Ok((package?, dashboard?))
}

async fn api_request<T: DeserializeOwned>(
    state: &WebState,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, ApiFailure> {
    let url = format!("{}{}", state.api_url.trim_end_matches('/'), path);
    let mut request = state
        .http
        .request(method, url)
        .bearer_auth(token)
        .header(header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(8));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| ApiFailure::Unavailable(error.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return response
            .json::<T>()
            .await
            .map_err(|error| ApiFailure::Unavailable(error.to_string()));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ApiFailure::Unauthorized);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ApiFailure::NotFound);
    }
    let message = response
        .json::<ApiErrorBody>()
        .await
        .ok()
        .and_then(|body| body.message)
        .unwrap_or_else(|| format!("registry API returned HTTP {}", status.as_u16()));
    Err(ApiFailure::Rejected {
        status: status.as_u16(),
        message,
    })
}

fn session_token(headers: &HeaderMap, state: &WebState) -> Option<String> {
    bearer(headers).or_else(|| cookie_value(headers, &state.session_cookie_name))
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(candidate, value)| {
            (candidate == name && !value.is_empty()).then(|| value.to_owned())
        })
}

fn require_same_origin(headers: &HeaderMap, state: &WebState) -> Result<(), Box<Response>> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if origin == Some(state.public_origin.trim_end_matches('/')) {
        Ok(())
    } else {
        Err(Box::new(simple_page(
            state,
            None,
            StatusCode::FORBIDDEN,
            "Request rejected",
            "The form origin did not match this registry console.",
        )))
    }
}

fn account_home_page(_state: &WebState, home: HomeResponse) -> Html<String> {
    let title = match home
        .user
        .as_ref()
        .and_then(|user| user.display_name.as_deref())
    {
        Some(name) => format!("Welcome, {name}"),
        None => "Your registry".to_owned(),
    };
    let user = home.user.as_ref();
    Html(
        account_layout(
            "Home - zed-pkg",
            user,
            html! {
                section class="console-hero" {
                    div {
                        p class="eyebrow" { "Registry home" }
                        h1 { (title) }
                        p class="muted" {
                            "Search packages and projects across every organization you can access."
                        }
                    }
                    a class="button" href="/dashboard" { "Open dashboard" }
                }
                form method="get" action="/" class="console-search" {
                    input type="search" name="q" value=(home.query) placeholder="Search projects and packages";
                    button type="submit" class="button primary" { "Search" }
                }
                section class="console-grid three" {
                    (metric_card("Organizations", home.orgs.len(), "/dashboard"))
                    (metric_card("Projects", home.projects.len(), "/dashboard"))
                    (metric_card("Packages", home.packages.len(), "/search"))
                }
                section class="console-grid two" {
                    div class="console-card" {
                        h2 { "Organizations" }
                        (org_list(&home.orgs))
                    }
                    div class="console-card" {
                        h2 { "Projects" }
                        (project_list(&home.projects))
                    }
                }
                section class="console-card" {
                    h2 { "Packages" }
                    (package_list(&home.packages))
                }
            },
        )
        .into_string(),
    )
}

fn dashboard_page(_state: &WebState, home: HomeResponse, error: Option<String>) -> Html<String> {
    let user = home.user.as_ref();
    Html(
        account_layout(
            "Dashboard - zed-pkg",
            user,
            html! {
                section class="console-hero" {
                    div {
                        p class="eyebrow" { "Dashboard" }
                        h1 { "Your organizations" }
                        p class="muted" { "Choose an organization or create a new registry workspace." }
                    }
                }
                (notice(error.as_deref()))
                section class="console-grid two" {
                    div class="console-card" {
                        h2 { "Organizations" }
                        (org_list(&home.orgs))
                    }
                    div class="console-card" {
                        h2 { "Create organization" }
                        form method="post" action="/dashboard" class="stack" {
                            label { "Slug" input name="slug" required maxlength="64" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]"; }
                            label { "Display name" input name="name" required maxlength="160"; }
                            button type="submit" class="button primary" { "Create organization" }
                        }
                    }
                }
            },
        )
        .into_string(),
    )
}

fn org_dashboard_page(
    _state: &WebState,
    dashboard: DashboardResponse,
    error: Option<String>,
) -> Html<String> {
    let org = dashboard.org;
    Html(
        account_layout(
            &format!("{} dashboard - zed-pkg", org.name),
            None,
            html! {
                (org_header(&org, "Dashboard"))
                (notice(error.as_deref()))
                section class="console-grid three" {
                    (metric_card("Projects", dashboard.projects.len(), &format!("/orgs/{}/settings", path_segment(&org.slug))))
                    (metric_card("Packages", dashboard.packages.len(), &format!("/orgs/{}/settings", path_segment(&org.slug))))
                    (metric_card("Role", 0, &format!("/orgs/{}/settings", path_segment(&org.slug))))
                }
                section class="console-grid two" {
                    div class="console-card" {
                        h2 { "Projects" }
                        (project_list(&dashboard.projects))
                    }
                    div class="console-card" {
                        h2 { "Create project" }
                        form method="post" action={ "/orgs/" (org.slug) "/dashboard" } class="stack" {
                            label { "Slug" input name="slug" required maxlength="64" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]"; }
                            label { "Display name" input name="name" required maxlength="160"; }
                            button type="submit" class="button primary" { "Create project" }
                        }
                    }
                }
                section class="console-card" {
                    h2 { "Packages" }
                    (package_list(&dashboard.packages))
                }
            },
        )
        .into_string(),
    )
}

fn org_settings_page(
    state: &WebState,
    dashboard: DashboardResponse,
    error: Option<String>,
    invitation: Option<InvitationResponse>,
) -> Html<String> {
    let org = dashboard.org;
    Html(
        account_layout(
            &format!("{} settings - zed-pkg", org.name),
            None,
            html! {
                (org_header(&org, "Organization settings"))
                (notice(error.as_deref()))
                (invitation_notice(state, invitation.as_ref()))
                section class="console-grid two" {
                    div class="console-card" {
                        h2 { "Organization" }
                        dl class="details" {
                            dt { "Slug" } dd class="mono" { (org.slug) }
                            dt { "Role" } dd { (org.role) }
                            dt { "Description" } dd { (org.description.as_deref().unwrap_or("No description")) }
                        }
                    }
                    div class="console-card" {
                        h2 { "Invite a member" }
                        form method="post" action={ "/orgs/" (org.slug) "/settings" } class="stack" {
                            label { "Email" input type="email" name="email" required maxlength="320"; }
                            label { "Role" select name="role" {
                                option value="member" { "Member" }
                                option value="reader" { "Reader" }
                                option value="admin" { "Administrator" }
                            } }
                            button type="submit" class="button primary" { "Create invitation" }
                        }
                    }
                }
                section class="console-card" {
                    h2 { "Projects" }
                    (project_list(&dashboard.projects))
                }
            },
        )
        .into_string(),
    )
}

fn project_settings_page(
    state: &WebState,
    project: ProjectResponse,
    error: Option<String>,
    invitation: Option<InvitationResponse>,
) -> Html<String> {
    let org_slug = project.org_slug.clone();
    let project_slug = project.slug.clone();
    Html(
        account_layout(
            &format!("{} settings - zed-pkg", project.name),
            None,
            html! {
                section class="console-hero" {
                    div {
                        p class="eyebrow" { (org_slug) " / project" }
                        h1 { (project.name) }
                        p class="muted" { (project.description.as_deref().unwrap_or("No project description")) }
                    }
                    a class="button" href={ "/orgs/" (org_slug) "/dashboard" } { "Organization dashboard" }
                }
                (notice(error.as_deref()))
                (invitation_notice(state, invitation.as_ref()))
                section class="console-grid two" {
                    div class="console-card" {
                        h2 { "Project" }
                        dl class="details" {
                            dt { "Slug" } dd class="mono" { (project_slug) }
                            dt { "Role" } dd { (project.role) }
                            dt { "Project ID" } dd class="mono" { (project.id) }
                        }
                    }
                    div class="console-card" {
                        h2 { "Invite a project member" }
                        form method="post" action={ "/orgs/" (org_slug) "/projects/" (project_slug) "/settings" } class="stack" {
                            label { "Email" input type="email" name="email" required maxlength="320"; }
                            label { "Role" select name="role" {
                                option value="member" { "Member" }
                                option value="reader" { "Reader" }
                                option value="admin" { "Administrator" }
                            } }
                            button type="submit" class="button primary" { "Create invitation" }
                        }
                    }
                }
            },
        )
        .into_string(),
    )
}

fn package_settings_page(
    _state: &WebState,
    package: PackageResponse,
    projects: Vec<ProjectResponse>,
    error: Option<String>,
) -> Html<String> {
    let config = serde_json::to_string_pretty(&package.config).unwrap_or_else(|_| "{}".into());
    let action = format!(
        "/orgs/{}/packages/{}/settings",
        path_segment(&package.org_slug),
        path_segment(&package.name)
    );
    Html(
        account_layout(
            &format!("{} settings - zed-pkg", package.name),
            None,
            html! {
                section class="console-hero" {
                    div {
                        p class="eyebrow" { (package.org_slug) " / package" }
                        h1 class="mono" { (package.name) }
                        p class="muted" { (package.repo_url) }
                    }
                    a class="button" href={ "/p/" (package.org_slug) "/" (package.name) } { "Public package page" }
                }
                (notice(error.as_deref()))
                section class="console-card" {
                    h2 { "Package configuration" }
                    form method="post" action=(action) class="stack" {
                        label { "Description" textarea name="description" maxlength="4000" { (package.description.unwrap_or_default()) } }
                        label { "Project" select name="project_id" {
                            option value="" { "No project" }
                            @for project in &projects {
                                option value=(project.id) selected[package.project_id.as_deref() == Some(project.id.as_str())] {
                                    (project.name)
                                }
                            }
                        } }
                        label { "Visibility" select name="visibility" {
                            @for visibility in ["public", "internal", "private"] {
                                option value=(visibility) selected[package.visibility == visibility] { (visibility) }
                            }
                        } }
                        label { "Configuration (JSON)" textarea name="config" class="mono code-area" { (config) } }
                        button type="submit" class="button primary" { "Save package settings" }
                    }
                }
            },
        )
        .into_string(),
    )
}

fn user_settings_page(
    _state: &WebState,
    user: UserResponse,
    error: Option<String>,
) -> Html<String> {
    let settings = serde_json::to_string_pretty(&user.settings).unwrap_or_else(|_| "{}".into());
    Html(
        account_layout(
            "User settings - zed-pkg",
            Some(&user),
            html! {
                section class="console-hero" {
                    div {
                        p class="eyebrow" { "User settings" }
                        h1 { (user.display_name.as_deref().or(user.email.as_deref()).unwrap_or("Registry user")) }
                        p class="muted mono" { (user.subject) }
                    }
                }
                (notice(error.as_deref()))
                section class="console-card narrow" {
                    form method="post" action="/settings" class="stack" {
                        label { "Display name" input name="display_name" maxlength="160" value=(user.display_name.as_deref().unwrap_or_default()); }
                        label { "Avatar URL" input type="url" name="avatar_url" maxlength="2048" value=(user.avatar_url.as_deref().unwrap_or_default()); }
                        label { "Settings (JSON)" textarea name="settings" class="mono code-area" { (settings) } }
                        button type="submit" class="button primary" { "Save user settings" }
                    }
                }
            },
        )
        .into_string(),
    )
}

fn account_layout(title: &str, user: Option<&UserResponse>, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/styles.css";
                link rel="stylesheet" href="/static/account.css";
                link rel="icon" type="image/svg+xml" href="/static/favicon.svg";
                script src="/static/htmx.min.js" {}
            }
            body class="console-body" {
                nav {
                    div class="wrap nav-inner" {
                        a class="brand" href="/" {
                            span class="brand-z" { "zed" } span class="brand-pkg" { "-pkg" }
                            span class="brand-tag" { "console" }
                        }
                        div class="nav-links" {
                            a href="/" { "Home" }
                            a href="/dashboard" { "Dashboard" }
                            a href="/settings" { "User settings" }
                            a href="/search" { "Public registry" }
                            @if user.is_none() {
                                a href="/login" { "Sign in" }
                            }
                        }
                    }
                }
                main class="wrap console-main" { (content) }
                footer {
                    div class="wrap" {
                        "zed-pkg account console - MASH (Maud, Axum, SeaORM, HTMX)"
                    }
                }
            }
        }
    }
}

fn org_header(org: &OrgResponse, context: &str) -> Markup {
    html! {
        section class="console-hero" {
            div {
                p class="eyebrow" { (context) }
                h1 { (org.name) }
                p class="muted" { (org.description.as_deref().unwrap_or("No organization description")) }
            }
            div class="button-row" {
                a class="button" href={ "/orgs/" (org.slug) "/dashboard" } { "Dashboard" }
                a class="button" href={ "/orgs/" (org.slug) "/settings" } { "Settings" }
            }
        }
    }
}

fn org_list(orgs: &[OrgResponse]) -> Markup {
    html! {
        @if orgs.is_empty() {
            p class="muted" { "No organizations yet." }
        } @else {
            ul class="console-list" {
                @for org in orgs {
                    li {
                        div {
                            a class="console-name" href={ "/orgs/" (org.slug) "/dashboard" } { (org.name) }
                            span class="muted mono" { (org.slug) }
                        }
                        span class="pill" { (org.role) }
                    }
                }
            }
        }
    }
}

fn project_list(projects: &[ProjectResponse]) -> Markup {
    html! {
        @if projects.is_empty() {
            p class="muted" { "No projects yet." }
        } @else {
            ul class="console-list" {
                @for project in projects {
                    li {
                        div {
                            a class="console-name" href={ "/orgs/" (project.org_slug) "/projects/" (project.slug) "/settings" } {
                                (project.name)
                            }
                            span class="muted mono" { (project.org_slug) "/" (project.slug) }
                        }
                        span class="pill" { (project.role) }
                    }
                }
            }
        }
    }
}

fn package_list(packages: &[PackageResponse]) -> Markup {
    html! {
        @if packages.is_empty() {
            p class="muted" { "No packages match this view." }
        } @else {
            ul class="console-list" {
                @for package in packages {
                    li {
                        div {
                            a class="console-name mono" href={ "/orgs/" (package.org_slug) "/packages/" (package.name) "/settings" } {
                                (package.org_slug) "/" (package.name)
                            }
                            @if let Some(description) = &package.description {
                                span class="muted" { (description) }
                            }
                        }
                        span class="pill" { (package.visibility) }
                    }
                }
            }
        }
    }
}

fn metric_card(label: &str, value: usize, href: &str) -> Markup {
    html! {
        a class="console-card metric" href=(href) {
            span class="eyebrow" { (label) }
            strong { (value) }
        }
    }
}

fn invitation_notice(state: &WebState, invitation: Option<&InvitationResponse>) -> Markup {
    html! {
        @if let Some(invitation) = invitation {
            div class="notice success" {
                strong { "Invitation created for " (invitation.email) "." }
                p { "Role: " (invitation.role) ". Share this one-time acceptance link securely:" }
                code class="invite-link" {
                    (state.public_origin.trim_end_matches('/'))
                    "/invitations/accept?token="
                    (invitation.token)
                }
                p class="muted" { "Invitation ID: " (invitation.invitation_id) }
            }
        }
    }
}

fn notice(message: Option<&str>) -> Markup {
    html! {
        @if let Some(message) = message {
            div class="notice error" { (message) }
        }
    }
}

fn api_failure_page(
    state: &WebState,
    user: Option<&UserResponse>,
    title: &str,
    error: ApiFailure,
) -> Response {
    let (status, message) = match error {
        ApiFailure::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "Your Shared Auth session is missing or expired.".to_owned(),
        ),
        ApiFailure::NotFound => (
            StatusCode::NOT_FOUND,
            "The requested resource was not found.".to_owned(),
        ),
        ApiFailure::Rejected { status, message } => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            message,
        ),
        ApiFailure::Unavailable(message) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("The registry account API is unavailable: {message}"),
        ),
    };
    simple_page(state, user, status, title, &message)
}

fn failure_message(error: ApiFailure) -> String {
    match error {
        ApiFailure::Unauthorized => "Your Shared Auth session expired.".into(),
        ApiFailure::NotFound => "The requested resource was not found.".into(),
        ApiFailure::Rejected { message, .. } | ApiFailure::Unavailable(message) => message,
    }
}

fn simple_page(
    _state: &WebState,
    user: Option<&UserResponse>,
    status: StatusCode,
    title: &str,
    message: &str,
) -> Response {
    (
        status,
        Html(
            account_layout(
                title,
                user,
                html! {
                    section class="console-card narrow" {
                        p class="eyebrow" { (status.as_u16()) }
                        h1 { (title) }
                        p class="muted" { (message) }
                        a class="button" href="/dashboard" { "Back to dashboard" }
                    }
                },
            )
            .into_string(),
        ),
    )
        .into_response()
}

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

fn login_redirect(return_to: &str) -> Response {
    Redirect::to(&format!(
        "/login?return_to={}",
        query_component(&safe_return_path(return_to))
    ))
    .into_response()
}

fn shared_auth_sign_in(return_to: &str) -> String {
    format!(
        "/shared-auth/auth/browser/sign-in?return={}",
        query_component(&safe_return_path(return_to))
    )
}

fn safe_return_path(value: &str) -> String {
    if value.starts_with('/')
        && !value.starts_with("//")
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
    {
        value.to_owned()
    } else {
        "/dashboard".to_owned()
    }
}

fn path_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn query_component(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_json_object(value: &str, label: &str) -> Result<Value, String> {
    let value = serde_json::from_str::<Value>(value)
        .map_err(|error| format!("Invalid {label}: {error}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(format!("{label} must be a JSON object"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_paths_fail_closed() {
        assert_eq!(safe_return_path("/dashboard"), "/dashboard");
        for unsafe_value in ["https://attacker.example", "//attacker.example", "/\\evil"] {
            assert_eq!(safe_return_path(unsafe_value), "/dashboard");
        }
    }

    #[test]
    fn exact_cookie_name_is_required() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=value; __Host-ore-session=token"),
        );
        assert_eq!(
            cookie_value(&headers, "__Host-ore-session").as_deref(),
            Some("token")
        );
        assert_eq!(cookie_value(&headers, "ore-session"), None);
    }

    #[test]
    fn settings_must_be_json_objects() {
        assert!(parse_json_object("{}", "settings").is_ok());
        assert!(parse_json_object("[]", "settings").is_err());
        assert!(parse_json_object("not json", "settings").is_err());
    }
}
