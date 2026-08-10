//! Token-blind session presence and refresh endpoints for the static zpkg.net site.
//!
//! The marketing origin can learn only whether an app session is valid. Shared
//! Auth refresh handles remain inside the signed, HttpOnly, host-only
//! `app.zpkg.net` cookie and bearer tokens never enter marketing JavaScript.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::state::{BrowserAuthConfig, WebState};

const MARKETING_ORIGIN: &str = "https://zpkg.net";
const SESSION_MAX_AGE_SECONDS: i64 = 30 * 24 * 60 * 60;
const SESSION_REFRESH_AFTER_SECONDS: u64 = 50 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserSession {
    refresh_token: String,
    shared_user_id: Uuid,
    issued_at: i64,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    refresh_token: String,
    shared_user_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarketingSessionStatus {
    authenticated: bool,
    refresh_after_seconds: u64,
}

/// Report session presence without returning a principal, token, tenant, or
/// account attribute.
pub async fn status(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(config) = state.browser_auth.as_ref() else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "browser authentication is not configured",
        );
    };
    if !marketing_origin_allowed(&headers, config) {
        return error_json(StatusCode::FORBIDDEN, "cross-origin session probe rejected");
    }

    let cookie_present = cookie_value(&headers, &config.session_cookie_name).is_some();
    let authenticated = read_signed_cookie::<BrowserSession>(
        &headers,
        &config.session_cookie_name,
        config,
    )
    .as_ref()
    .is_some_and(session_is_current);
    let mut response = marketing_status_response(config, &headers, StatusCode::OK, authenticated);
    if cookie_present && !authenticated {
        append_cookie(
            &mut response,
            clear_cookie(&config.session_cookie_name, config),
        );
    }
    response
}

/// Rotate the opaque Shared Auth refresh handle inside the product cookie.
/// Neither the handle nor the resulting access token is returned to the caller.
pub async fn refresh(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(config) = state.browser_auth.as_ref() else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "browser authentication is not configured",
        );
    };
    if !marketing_origin_allowed(&headers, config) {
        return error_json(
            StatusCode::FORBIDDEN,
            "cross-origin session refresh rejected",
        );
    }

    let cookie_present = cookie_value(&headers, &config.session_cookie_name).is_some();
    let Some(session) = read_signed_cookie::<BrowserSession>(
        &headers,
        &config.session_cookie_name,
        config,
    )
    .filter(session_is_current)
    else {
        let mut response = marketing_status_response(config, &headers, StatusCode::OK, false);
        if cookie_present {
            append_cookie(
                &mut response,
                clear_cookie(&config.session_cookie_name, config),
            );
        }
        return response;
    };

    let upstream = state
        .http
        .post(format!("{}/auth/refresh", config.shared_auth_url))
        .json(&json!({ "refresh_token": session.refresh_token }))
        .send()
        .await;
    let upstream = match upstream {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "Shared Auth session refresh failed");
            return marketing_status_response(
                config,
                &headers,
                StatusCode::SERVICE_UNAVAILABLE,
                false,
            );
        }
    };

    let upstream_status = upstream.status();
    if !upstream_status.is_success() {
        if matches!(
            upstream_status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            tracing::warn!(status = %upstream_status, "Shared Auth session refresh rejected");
            let mut response = marketing_status_response(config, &headers, StatusCode::OK, false);
            append_cookie(
                &mut response,
                clear_cookie(&config.session_cookie_name, config),
            );
            return response;
        }
        tracing::warn!(status = %upstream_status, "Shared Auth session refresh unavailable");
        return marketing_status_response(config, &headers, StatusCode::SERVICE_UNAVAILABLE, false);
    }

    let refreshed = match upstream.json::<RefreshResponse>().await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Shared Auth refresh response was invalid");
            return marketing_status_response(
                config,
                &headers,
                StatusCode::SERVICE_UNAVAILABLE,
                false,
            );
        }
    };
    if refreshed.shared_user_id != session.shared_user_id {
        tracing::warn!("Shared Auth refresh changed the canonical principal");
        let mut response = marketing_status_response(config, &headers, StatusCode::OK, false);
        append_cookie(
            &mut response,
            clear_cookie(&config.session_cookie_name, config),
        );
        return response;
    }

    let next_session = BrowserSession {
        refresh_token: refreshed.refresh_token,
        shared_user_id: refreshed.shared_user_id,
        issued_at: Utc::now().timestamp(),
    };
    let mut response = marketing_status_response(config, &headers, StatusCode::OK, true);
    append_cookie(
        &mut response,
        signed_cookie(
            &config.session_cookie_name,
            &next_session,
            SESSION_MAX_AGE_SECONDS,
            config,
        ),
    );
    response
}

fn session_is_current(session: &BrowserSession) -> bool {
    let age = Utc::now().timestamp() - session.issued_at;
    (0..=SESSION_MAX_AGE_SECONDS).contains(&age)
}

fn marketing_origin_allowed(headers: &HeaderMap, config: &BrowserAuthConfig) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == MARKETING_ORIGIN || origin == config.public_origin)
}

fn marketing_status_response(
    config: &BrowserAuthConfig,
    request_headers: &HeaderMap,
    status: StatusCode,
    authenticated: bool,
) -> Response {
    let mut response = (
        status,
        axum::Json(MarketingSessionStatus {
            authenticated,
            refresh_after_seconds: SESSION_REFRESH_AFTER_SECONDS,
        }),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    headers.insert(
        HeaderName::from_static("pragma"),
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-site"),
    );
    headers.append(header::VARY, HeaderValue::from_static("Origin"));

    if request_headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(MARKETING_ORIGIN)
    {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static(MARKETING_ORIGIN),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }

    debug_assert!(marketing_origin_allowed(request_headers, config));
    response
}

fn signed_cookie<T: Serialize>(
    name: &str,
    value: &T,
    max_age: i64,
    config: &BrowserAuthConfig,
) -> String {
    let encoded = serde_json::to_vec(value).expect("session structs are JSON serializable");
    let payload = hex_encode(&encoded);
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
    fn status_payload_is_token_blind_and_uses_fifty_minutes() {
        let payload = serde_json::to_string(&MarketingSessionStatus {
            authenticated: true,
            refresh_after_seconds: SESSION_REFRESH_AFTER_SECONDS,
        })
        .unwrap();
        assert_eq!(
            payload,
            "{\"authenticated\":true,\"refreshAfterSeconds\":3000}"
        );
        for forbidden in ["token", "email", "subject", "tenant"] {
            assert!(!payload.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn marketing_origin_is_exact_and_never_wildcarded() {
        let config = config();
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static(MARKETING_ORIGIN));
        assert!(marketing_origin_allowed(&headers, &config));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.zpkg.net"),
        );
        assert!(!marketing_origin_allowed(&headers, &config));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.zpkg.net"),
        );
        assert!(marketing_origin_allowed(&headers, &config));
    }

    #[test]
    fn credentialed_cors_is_emitted_only_for_the_marketing_origin() {
        let config = config();
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static(MARKETING_ORIGIN));
        let response = marketing_status_response(&config, &headers, StatusCode::OK, true);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static(MARKETING_ORIGIN))
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&HeaderValue::from_static("true"))
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store, max-age=0"))
        );
    }

    #[test]
    fn signed_cookie_round_trips_and_rejects_tampering() {
        let config = config();
        let session = BrowserSession {
            refresh_token: "refresh".into(),
            shared_user_id: Uuid::nil(),
            issued_at: Utc::now().timestamp(),
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
    fn hmac_matches_the_rfc_4231_case_one_vector() {
        assert_eq!(
            hex_encode(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
