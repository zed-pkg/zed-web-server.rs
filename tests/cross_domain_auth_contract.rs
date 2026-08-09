const BROWSER_AUTH_SOURCE: &str = include_str!("../src/browser_auth.rs");
const BFF_CONTRACT: &str = include_str!("../docs/shared-auth-bff.md");

fn function_source<'a>(source: &'a str, start: &str, next: &str) -> &'a str {
    let start_index = source.find(start).expect("contract function must exist");
    let tail = &source[start_index..];
    let end_index = tail
        .find(next)
        .expect("following contract function must exist");
    &tail[..end_index]
}

#[test]
fn browser_redirect_and_backchannel_use_distinct_shared_auth_origins() {
    assert!(BROWSER_AUTH_SOURCE.contains("config.shared_auth_public_url"));
    assert!(BROWSER_AUTH_SOURCE.contains("/authorize"));
    assert!(BROWSER_AUTH_SOURCE.contains("config.shared_auth_url"));
    assert!(BROWSER_AUTH_SOURCE.contains("/auth/handoff/redeem"));
    assert!(BROWSER_AUTH_SOURCE.contains("/auth/exchange"));
    assert!(BROWSER_AUTH_SOURCE.contains("/auth/delegate"));
}

#[test]
fn callback_is_anchored_to_the_zpkg_product_origin() {
    let callback = function_source(
        BROWSER_AUTH_SOURCE,
        "fn callback_uri",
        "fn sanitize_return_to",
    );
    assert!(callback.contains("config.public_origin"));
    assert!(callback.contains("/auth/shared/callback"));
    assert!(!callback.contains("shared_auth_public_url"));
    assert!(!callback.contains("shared_auth_url"));
}

#[test]
fn product_cookie_is_host_only_and_never_sets_a_parent_domain() {
    let cookie = function_source(BROWSER_AUTH_SOURCE, "fn signed_cookie", "fn clear_cookie");
    assert!(cookie.contains("Path=/"));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("; Secure"));
    assert!(!cookie.contains("Domain="));
}

#[test]
fn written_contract_rejects_cookie_sharing_and_direct_supabase_callbacks() {
    assert!(BFF_CONTRACT.contains("Shared Auth -> Zpkg"));
    assert!(BFF_CONTRACT.contains("not a direct Supabase callback"));
    assert!(BFF_CONTRACT.contains("Never set `Domain=.zpkg.net`"));
    assert!(BFF_CONTRACT.contains("SHARED_AUTH_PUBLIC_URL=https://auth.oresoftware.dev"));
    let internal_url = concat!(
        "SHARED_AUTH_URL=http://",
        "shared-auth-server.shared-auth.svc.cluster.local",
    );
    assert!(BFF_CONTRACT.contains(internal_url));
}
