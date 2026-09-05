//! Account enrollment is a browser flow over the existing API-owned namespace
//! operation. No identity, membership, or database write originates here.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::browser_auth::{self, BrowserMutation, CreateOrgForm};
use crate::session::{self, Viewer};
use crate::state::WebState;
use crate::views::onboarding::{self, EnrollmentError, FormValues, Journey, Stage};
use crate::views::{PageContext, layout};

pub(super) fn router() -> Router<Arc<WebState>> {
    Router::new()
        .route("/onboarding", get(choose))
        .route(
            Journey::Individual.path(),
            get(individual).post(enroll_individual),
        )
        .route(
            Journey::Organization.path(),
            get(organization).post(enroll_organization),
        )
        .layer(DefaultBodyLimit::max(4096))
        // Apply outside extraction so malformed and oversized forms receive
        // the same cache policy as successfully rendered account pages.
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::VARY,
            HeaderValue::from_static("Cookie"),
        ))
}

pub async fn choose(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let viewer = session::resolve(&state, &headers).await;
    let response = Html(
        layout(
            "Get started",
            state.db.is_some(),
            &viewer,
            &PageContext::none(),
            onboarding::choose(),
        )
        .into_string(),
    )
    .into_response();
    private_response(response)
}

pub async fn individual(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    render(&state, &headers, Journey::Individual, None, None).await
}

pub async fn organization(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    render(&state, &headers, Journey::Organization, None, None).await
}

pub async fn enroll_individual(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<CreateOrgForm>,
) -> Response {
    enroll(state, headers, form, Journey::Individual).await
}

pub async fn enroll_organization(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Form(form): Form<CreateOrgForm>,
) -> Response {
    enroll(state, headers, form, Journey::Organization).await
}

async fn render(
    state: &WebState,
    headers: &HeaderMap,
    journey: Journey,
    failure: Option<(StatusCode, EnrollmentError)>,
    form: Option<&CreateOrgForm>,
) -> Response {
    let account = if state.browser_auth.is_none() || state.db.is_none() {
        Err(session::AccountResolutionError::DatabaseUnavailable)
    } else {
        session::resolve_account(state, headers).await
    };
    let viewer = account.as_ref().unwrap_or(&Viewer::Anonymous);
    let (status, stage) = match &account {
        Ok(Viewer::Anonymous) => (StatusCode::OK, Stage::SignIn),
        Ok(Viewer::SignedIn(viewer)) => (StatusCode::OK, Stage::Active(viewer)),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, Stage::Unavailable),
    };
    let values = form.map_or_else(FormValues::default, |form| FormValues {
        slug: &form.slug,
        name: &form.name,
    });
    let content = onboarding::page(journey, stage, failure.map(|(_, error)| error), values);
    let response = (
        failure.map_or(status, |(status, _)| status),
        Html(
            layout(
                journey.title(),
                state.db.is_some(),
                viewer,
                &PageContext::none(),
                content,
            )
            .into_string(),
        ),
    )
        .into_response();
    private_response(response)
}

async fn enroll(
    state: Arc<WebState>,
    headers: HeaderMap,
    form: CreateOrgForm,
    journey: Journey,
) -> Response {
    // The canonical BFF performs origin verification, cookie validation,
    // refresh, scoped delegation, and API submission. Its rotated cookie must
    // survive both successful submissions and rejected API responses.
    let outcome = browser_auth::create_org_outcome(&state, &headers, form.clone()).await;
    let upstream_headers = match &outcome {
        BrowserMutation::Applied(response)
        | BrowserMutation::SignIn(response)
        | BrowserMutation::Failed(response) => response.headers().clone(),
    };
    let mut response = match outcome {
        BrowserMutation::Applied(_) => Redirect::to(journey.path()).into_response(),
        BrowserMutation::SignIn(_) => Redirect::to(journey.sign_in_path()).into_response(),
        BrowserMutation::Failed(response) if response.status() == StatusCode::UNAUTHORIZED => {
            Redirect::to(journey.sign_in_path()).into_response()
        }
        BrowserMutation::Failed(response) => {
            let (status, error) = enrollment_failure(response.status());
            render(
                &state,
                &headers,
                journey,
                Some((status, error)),
                Some(&form),
            )
            .await
        }
    };
    for cookie in upstream_headers.get_all(header::SET_COOKIE) {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie.clone());
    }
    if let Some(retry) = upstream_headers.get(header::RETRY_AFTER) {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, retry.clone());
    }
    private_response(response)
}

fn enrollment_failure(status: StatusCode) -> (StatusCode, EnrollmentError) {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            (status, EnrollmentError::Invalid)
        }
        StatusCode::CONFLICT => (status, EnrollmentError::Conflict),
        StatusCode::FORBIDDEN => (status, EnrollmentError::Denied),
        status if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS => {
            (status, EnrollmentError::Unavailable)
        }
        _ => (StatusCode::BAD_GATEWAY, EnrollmentError::Unavailable),
    }
}

pub(super) fn private_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Cookie"));
    response
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    fn offline_state() -> Arc<WebState> {
        Arc::new(WebState {
            db: None,
            registry_url: String::new(),
            shared_auth_url: None,
            session_path: String::new(),
            browser_auth: None,
            http: reqwest::Client::new(),
        })
    }

    #[tokio::test]
    async fn chooser_and_each_unavailable_journey_are_routable_and_not_cacheable() {
        for (path, expected) in [
            ("/onboarding", StatusCode::OK),
            (Journey::Individual.path(), StatusCode::SERVICE_UNAVAILABLE),
            (
                Journey::Organization.path(),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ] {
            let response = crate::routes::router(offline_state())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            assert_eq!(response.headers()[header::VARY], "Cookie");
            let html = String::from_utf8(
                to_bytes(response.into_body(), 128 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            if path == "/onboarding" {
                assert!(html.contains(Journey::Individual.path()));
                assert!(html.contains(Journey::Organization.path()));
            } else {
                assert!(html.contains("Account setup is temporarily unavailable"));
                assert!(!html.contains("name=\"slug\""));
            }
        }
    }

    #[tokio::test]
    async fn oversized_enrollment_is_rejected_before_authentication_or_api_calls() {
        let response = crate::routes::router(offline_state())
            .oneshot(
                Request::post(Journey::Organization.path())
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("slug=acme&name={}", "x".repeat(8192))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
    }

    #[test]
    fn a_failed_mutation_can_never_become_success_from_its_http_status() {
        for code in 100..600 {
            let status = StatusCode::from_u16(code).unwrap();
            let (mapped, _) = enrollment_failure(status);
            assert!(
                mapped.is_client_error() || mapped.is_server_error(),
                "{status}"
            );
        }
        assert_eq!(
            enrollment_failure(StatusCode::CONFLICT),
            (StatusCode::CONFLICT, EnrollmentError::Conflict)
        );
    }
}
