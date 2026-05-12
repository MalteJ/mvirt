//! HTTP integration tests for ServiceAccount + StaticApiKey (ADR-0004).
//!
//! Spins a single-node cplane via the shared `TestServer` helper, walks
//! the full lifecycle of an SA + key, and exercises the auth-middleware
//! Bearer fork against a project-scoped read endpoint.

mod common;

use common::TestServer;
use serde_json::Value;

const PROJECT_SLUG: &str = "ci";

/// Create the Project the tests live in. The default Org is bootstrapped
/// by `TestServer::spawn`.
async fn ensure_project(server: &TestServer) {
    let resp = server
        .post_json(
            &format!("/orgs/{}/projects", TestServer::DEFAULT_ORG_SLUG),
            &serde_json::json!({"slug": PROJECT_SLUG, "name": "CI Project"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "create project failed");
}

async fn auth_get(server: &TestServer, path: &str, bearer: &str) -> reqwest::Response {
    server
        .client
        .get(format!("{}{}", server.base_url(), path))
        .bearer_auth(bearer)
        .send()
        .await
        .expect("request failed")
}

#[tokio::test]
async fn service_account_lifecycle_via_rest() {
    let server = TestServer::spawn().await;
    ensure_project(&server).await;

    // Create SA.
    let resp = server
        .post_json(
            &format!("/projects/{}/service-accounts", PROJECT_SLUG),
            &serde_json::json!({"name": "github-actions", "description": "CI"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let sa: Value = resp.json().await.unwrap();
    let sa_id = sa["id"].as_str().unwrap().to_string();
    assert_eq!(sa["projectSlug"], PROJECT_SLUG);
    assert_eq!(sa["name"], "github-actions");

    // Listing includes the new account.
    let resp = server
        .get(&format!("/projects/{}/service-accounts", PROJECT_SLUG))
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["serviceAccounts"].as_array().unwrap().len(), 1);

    // Duplicate name in the same project is rejected.
    let resp = server
        .post_json(
            &format!("/projects/{}/service-accounts", PROJECT_SLUG),
            &serde_json::json!({"name": "github-actions"}),
        )
        .await;
    assert_eq!(resp.status(), 409);

    // Delete it again, list goes back to empty.
    let resp = server
        .delete(&format!(
            "/projects/{}/service-accounts/{}",
            PROJECT_SLUG, sa_id
        ))
        .await;
    assert_eq!(resp.status(), 204);

    let resp = server
        .get(&format!("/projects/{}/service-accounts", PROJECT_SLUG))
        .await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["serviceAccounts"].as_array().unwrap().len(), 0);

    server.shutdown().await;
}

#[tokio::test]
async fn static_api_key_authenticates_against_project_endpoint() {
    let server = TestServer::spawn().await;
    ensure_project(&server).await;

    // Make an SA.
    let resp = server
        .post_json(
            &format!("/projects/{}/service-accounts", PROJECT_SLUG),
            &serde_json::json!({"name": "deploy-bot"}),
        )
        .await;
    let sa: Value = resp.json().await.unwrap();
    let sa_id = sa["id"].as_str().unwrap().to_string();

    // Mint an API key. The plaintext appears once on creation.
    let resp = server
        .post_json(
            &format!(
                "/projects/{}/service-accounts/{}/api-keys",
                PROJECT_SLUG, sa_id
            ),
            &serde_json::json!({"description": "test"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let key: Value = resp.json().await.unwrap();
    let key_id = key["id"].as_str().unwrap().to_string();
    let secret = key["secret"]
        .as_str()
        .expect("secret must be present on create response")
        .to_string();
    assert!(
        secret.starts_with("mvirt_sa_"),
        "expected secret to be a full bearer string, got {}",
        secret
    );

    // Subsequent GETs hide the plaintext.
    let resp = server
        .get(&format!(
            "/projects/{}/service-accounts/{}/api-keys",
            PROJECT_SLUG, sa_id
        ))
        .await;
    let body: Value = resp.json().await.unwrap();
    let keys = body["apiKeys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].get("secret").is_none() || keys[0]["secret"].is_null());

    // Use the bearer against the project endpoint — auth must accept it.
    let resp = auth_get(
        &server,
        &format!("/projects/{}/service-accounts", PROJECT_SLUG),
        &secret,
    )
    .await;
    assert_eq!(resp.status(), 200, "valid SA bearer should authorise");

    // Wrong secret with a valid id → 401.
    let mut tampered = secret.clone();
    let last = tampered.pop().unwrap();
    let bumped = if last == 'A' { 'B' } else { 'A' };
    tampered.push(bumped);
    let resp = auth_get(
        &server,
        &format!("/projects/{}/service-accounts", PROJECT_SLUG),
        &tampered,
    )
    .await;
    assert_eq!(resp.status(), 401, "tampered secret must 401");

    // Bearer with the right format but a non-existent id → 401.
    let bogus = format!("mvirt_sa_key_doesnotexist.{}", "AAAAAAAAAAAAAAAAAAAA");
    let resp = auth_get(
        &server,
        &format!("/projects/{}/service-accounts", PROJECT_SLUG),
        &bogus,
    )
    .await;
    assert_eq!(resp.status(), 401);

    // Malformed bearer (no `.` separator) → 401.
    let resp = auth_get(
        &server,
        &format!("/projects/{}/service-accounts", PROJECT_SLUG),
        "mvirt_sa_no_separator_here",
    )
    .await;
    assert_eq!(resp.status(), 401);

    // Revoke the key, then the same bearer that worked above must 401.
    let resp = server
        .delete(&format!(
            "/projects/{}/service-accounts/{}/api-keys/{}",
            PROJECT_SLUG, sa_id, key_id
        ))
        .await;
    assert_eq!(resp.status(), 204);

    let resp = auth_get(
        &server,
        &format!("/projects/{}/service-accounts", PROJECT_SLUG),
        &secret,
    )
    .await;
    assert_eq!(resp.status(), 401, "revoked SA bearer must 401");

    server.shutdown().await;
}

#[tokio::test]
async fn expired_static_api_key_is_rejected() {
    let server = TestServer::spawn().await;
    ensure_project(&server).await;

    let resp = server
        .post_json(
            &format!("/projects/{}/service-accounts", PROJECT_SLUG),
            &serde_json::json!({"name": "expiry-test"}),
        )
        .await;
    let sa: Value = resp.json().await.unwrap();
    let sa_id = sa["id"].as_str().unwrap().to_string();

    // Mint a key whose RFC3339 expiry is firmly in the past.
    let resp = server
        .post_json(
            &format!(
                "/projects/{}/service-accounts/{}/api-keys",
                PROJECT_SLUG, sa_id
            ),
            &serde_json::json!({"expiresAt": "2020-01-01T00:00:00Z"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let key: Value = resp.json().await.unwrap();
    let secret = key["secret"].as_str().unwrap().to_string();

    let resp = auth_get(
        &server,
        &format!("/projects/{}/service-accounts", PROJECT_SLUG),
        &secret,
    )
    .await;
    assert_eq!(resp.status(), 401, "expired SA bearer must 401");

    server.shutdown().await;
}
