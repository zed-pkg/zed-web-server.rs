//! Refine the finite model with real loopback HTTP calls, real cookie
//! validation, and the production BFF. No alternate mutation reducer is used.

use super::tests::{config, continuity_state, session_headers};
use super::*;
use axum::{Json, Router, routing::post};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scenario {
    same_origin: bool,
    valid_session: bool,
    refresh_result: u8,
    delegation_ok: bool,
    api_result: u8,
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    outcome: &'static str,
    calls: [usize; 3],
    returned_cookie: bool,
    verified_cookie: bool,
}

async fn invoke(case: Scenario) -> Observation {
    let calls = Arc::new(std::array::from_fn::<_, 3, _>(|_| AtomicUsize::new(0)));
    let refresh_calls = calls.clone();
    let delegate_calls = calls.clone();
    let api_calls = calls.clone();
    let app = Router::new()
        .route("/auth/refresh", post(move |Json(body): Json<Value>| {
            let calls = refresh_calls.clone();
            async move {
                calls[0].fetch_add(1, Ordering::SeqCst);
                assert_eq!(body, json!({"refresh_token": "opaque-refresh"}));
                match case.refresh_result {
                    0 | 2 => Json(json!({
                        "access_token": "base-access",
                        "refresh_token": "next-opaque-refresh",
                        "shared_user_id": if case.refresh_result == 0 { Uuid::nil() } else { Uuid::from_u128(1) },
                    })).into_response(),
                    1 => StatusCode::UNAUTHORIZED.into_response(),
                    _ => panic!("unsupported model refresh outcome"),
                }
            }
        }))
        .route("/auth/delegate", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let calls = delegate_calls.clone();
            async move {
                calls[1].fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers[header::AUTHORIZATION], "Bearer base-access");
                assert_eq!(body, json!({"client_id":"zpkg-web", "audience":"zed-pkg", "scopes":["zpkg:account"]}));
                if case.delegation_ok {
                    Json(json!({"access_token": "delegated"})).into_response()
                } else {
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }
        }))
        .route("/api/v1/account/orgs", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let calls = api_calls.clone();
            async move {
                calls[2].fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers[header::AUTHORIZATION], "Bearer delegated");
                assert_eq!(body, json!({"slug":"acme", "name":"Acme", "description":null, "settings":{}}));
                match case.api_result {
                    0 => StatusCode::CREATED.into_response(),
                    1 => (StatusCode::CONFLICT, "private upstream failure detail").into_response(),
                    2 => Redirect::to("http://127.0.0.1:9/must-not-follow").into_response(),
                    _ => panic!("unsupported model API outcome"),
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = config();
    config.shared_auth_url = origin.clone();
    config.api_url = origin;
    let mut headers = if case.valid_session {
        session_headers(&config, 10)
    } else {
        HeaderMap::new()
    };
    headers.insert(
        header::ORIGIN,
        if case.same_origin {
            "https://app.zpkg.net"
        } else {
            "https://attacker.example"
        }
        .parse()
        .unwrap(),
    );
    let state = continuity_state(config.clone());
    let (outcome, response) = match create_org_outcome(
        &state,
        &headers,
        CreateOrgForm {
            slug: "acme".into(),
            name: "Acme".into(),
        },
    )
    .await
    {
        BrowserMutation::Applied(response) => ("Applied", response),
        BrowserMutation::SignIn(response) => ("SignIn", response),
        BrowserMutation::Failed(response) => ("Failed", response),
    };
    let returned_cookie = response.headers().contains_key(header::SET_COOKIE);
    let mut verified_cookie = false;
    if returned_cookie {
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            cookie.split(';').next().unwrap().parse().unwrap(),
        );
        let (session, _) = active_session(&headers, &config).unwrap();
        assert_eq!(session.shared_user_id, Uuid::nil());
        assert_eq!(session.refresh_token, "next-opaque-refresh");
        verified_cookie = session.verified_at.is_some();
    }
    let body = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    for forbidden in [
        "private upstream failure detail",
        "opaque-refresh",
        "base-access",
        "delegated",
    ] {
        assert!(!body.contains(forbidden));
    }
    let observation = Observation {
        outcome,
        calls: std::array::from_fn(|index| calls[index].load(Ordering::SeqCst)),
        returned_cookie,
        verified_cookie,
    };
    server.abort();
    observation
}

#[tokio::test]
async fn every_finite_mutation_outcome_preserves_authority_and_rotation() {
    let mut checked = 0;
    for same_origin in [false, true] {
        for valid_session in [false, true] {
            for refresh_result in 0..3 {
                for delegation_ok in [false, true] {
                    for api_result in 0..3 {
                        let case = Scenario {
                            same_origin,
                            valid_session,
                            refresh_result,
                            delegation_ok,
                            api_result,
                        };
                        let refresh = same_origin && valid_session;
                        let delegate = refresh && refresh_result == 0;
                        let api = delegate && delegation_ok;
                        let outcome = if !same_origin {
                            "Failed"
                        } else if !valid_session {
                            "SignIn"
                        } else if api && api_result == 0 {
                            "Applied"
                        } else {
                            "Failed"
                        };
                        assert_eq!(
                            invoke(case).await,
                            Observation {
                                outcome,
                                calls: [
                                    usize::from(refresh),
                                    usize::from(delegate),
                                    usize::from(api)
                                ],
                                returned_cookie: delegate,
                                verified_cookie: api,
                            },
                            "{case:?}"
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 72);
}

#[tokio::test]
async fn invalid_session_lifetimes_are_rejected_by_every_authority_path() {
    let mut config = config();
    config.shared_auth_url = "http://127.0.0.1:9".into();
    config.api_url = "http://127.0.0.1:9".into();
    let now = Utc::now().timestamp();
    for issued_at in [
        now - SESSION_MAX_AGE_SECONDS - 60,
        now + 60,
        i64::MIN,
        i64::MAX,
    ] {
        let cookie = signed_cookie(
            &config.session_cookie_name,
            &BrowserSession {
                shared_user_id: Uuid::nil(),
                refresh_token: "opaque-refresh".into(),
                issued_at,
                verified_at: Some(issued_at),
            },
            SESSION_MAX_AGE_SECONDS,
            &config,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            cookie.split(';').next().unwrap().parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "https://app.zpkg.net".parse().unwrap());
        let state = continuity_state(config.clone());
        assert!(session_subject(&state, &headers).is_none());
        assert!(matches!(
            create_org_outcome(
                &state,
                &headers,
                CreateOrgForm {
                    slug: "acme".into(),
                    name: "Acme".into(),
                }
            )
            .await,
            BrowserMutation::SignIn(_)
        ));
        let read = delegated_get(
            &state,
            &headers,
            Method::GET,
            "http://127.0.0.1:9/api/v1/account/me".parse().unwrap(),
            "application/json",
        )
        .await;
        assert!(matches!(read, Err(response) if response.status() == StatusCode::UNAUTHORIZED));
        assert!(matches!(
            verify_session_continuity(&state, &headers, 1).await,
            SessionContinuity::Anonymous(Some(_))
        ));
    }
}

#[tokio::test]
async fn delegation_outage_preserves_rotation_on_the_session_status_route() {
    use tower::ServiceExt;
    let app = Router::new()
        .route("/auth/refresh", post(|| async { Json(json!({
            "access_token":"base-access", "refresh_token":"next-opaque-refresh", "shared_user_id":Uuid::nil(),
        })) }))
        .route("/auth/delegate", post(|| async { StatusCode::SERVICE_UNAVAILABLE }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = config();
    config.shared_auth_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let headers = session_headers(&config, 3600);
    let response = crate::routes::router(Arc::new(continuity_state(config.clone())))
        .oneshot(
            axum::http::Request::get("/auth/session/status")
                .header(header::COOKIE, headers[header::COOKIE].clone())
                .header(header::ORIGIN, "https://zpkg.net")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let mut next_headers = HeaderMap::new();
    next_headers.insert(
        header::COOKIE,
        cookie.split(';').next().unwrap().parse().unwrap(),
    );
    let (session, _) = active_session(&next_headers, &config).unwrap();
    assert_eq!(session.refresh_token, "next-opaque-refresh");
    assert_eq!(session.verified_at, None);
    // A fresh refresh handle is not fresh delegation evidence. This second
    // probe must call the authority again, not take the coarse cached fast path.
    assert!(matches!(
        verify_session_continuity(&continuity_state(config), &next_headers, 3000,).await,
        SessionContinuity::Unavailable(Some(_))
    ));
    server.abort();
}

#[tokio::test]
async fn legacy_and_invalid_verification_times_never_shortcut_delegation() {
    let mut config = config();
    config.shared_auth_url = "http://127.0.0.1:9".into();
    let now = Utc::now().timestamp();
    for verified_at in [
        None,
        Some(now - 60),
        Some(now + 60),
        Some(i64::MIN),
        Some(i64::MAX),
    ] {
        let cookie = signed_cookie(
            &config.session_cookie_name,
            &BrowserSession {
                shared_user_id: Uuid::nil(),
                refresh_token: "opaque-refresh".into(),
                issued_at: now,
                verified_at,
            },
            SESSION_MAX_AGE_SECONDS,
            &config,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            cookie.split(';').next().unwrap().parse().unwrap(),
        );
        assert!(matches!(
            verify_session_continuity(&continuity_state(config.clone()), &headers, 3000,).await,
            SessionContinuity::Unavailable(None)
        ));
    }
}

fn model_bool(state: &Value, key: &str) -> bool {
    state[key].as_bool().expect("model boolean is required")
}

fn model_digit(state: &Value, key: &str) -> u8 {
    match state[key]["#bigint"]
        .as_str()
        .expect("ITF integer is required")
    {
        "0" => 0,
        "1" => 1,
        "2" => 2,
        _ => panic!("model integer is outside the finite domain"),
    }
}

fn model_tag<'a>(state: &'a Value, key: &str) -> &'a str {
    assert_eq!(state[key]["value"], json!({"#tup":[]}));
    state[key]["tag"].as_str().expect("ITF variant is required")
}

fn scenario_from_model(state: &Value) -> Scenario {
    Scenario {
        same_origin: model_bool(state, "same_origin"),
        valid_session: model_bool(state, "valid_session"),
        refresh_result: model_digit(state, "refresh_result"),
        delegation_ok: model_bool(state, "delegation_ok"),
        api_result: model_digit(state, "api_result"),
    }
}

// fmctl supplies generated traces. Never silently skip when no corpus exists.
#[tokio::test]
#[ignore = "executed by fmctl replay after trace generation"]
async fn replay_generated_model_traces() {
    let paths: Vec<String> = serde_json::from_str(
        &std::env::var("ZED_FM_TRACES").expect("fmctl must provide the trace set"),
    )
    .unwrap();
    assert!(!paths.is_empty() && paths.len() <= 64);
    for path in paths {
        assert!(std::fs::metadata(&path).unwrap().len() <= 1024 * 1024);
        let trace: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let states = trace["states"].as_array().expect("ITF states are required");
        assert!((2..=9).contains(&states.len()));
        assert_eq!(states[0]["mbt::actionTaken"], "init");
        assert_eq!(model_tag(&states[0]["s"], "phase"), "Origin");
        assert_eq!(model_tag(&states[0]["s"], "outcome"), "Pending");
        let case = scenario_from_model(&states[0]["s"]);
        for state in states {
            assert_eq!(scenario_from_model(&state["s"]), case);
            assert!(matches!(
                model_tag(&state["s"], "phase"),
                "Origin" | "Session" | "Refresh" | "Delegate" | "Api" | "Done"
            ));
            assert!(matches!(
                model_tag(&state["s"], "outcome"),
                "Pending" | "Applied" | "SignIn" | "Failed"
            ));
        }
        let final_state = &states.last().unwrap()["s"];
        assert_eq!(
            model_tag(final_state, "phase"),
            "Done",
            "trace must reach its terminal observation"
        );
        let observed = invoke(case).await;
        assert_eq!(
            observed.outcome,
            model_tag(final_state, "outcome"),
            "{path}"
        );
        assert_eq!(
            observed.calls,
            [
                usize::from(model_bool(final_state, "refresh_called")),
                usize::from(model_bool(final_state, "delegate_called")),
                usize::from(model_bool(final_state, "api_called")),
            ],
            "{path}"
        );
        assert_eq!(
            observed.returned_cookie,
            model_bool(final_state, "returned_cookie"),
            "{path}"
        );
        assert_eq!(
            observed.verified_cookie,
            model_bool(final_state, "verified_cookie"),
            "{path}"
        );
    }
}
