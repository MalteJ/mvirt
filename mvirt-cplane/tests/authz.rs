//! Account + Membership integration tests (ADR-0004).
//!
//! These run against TestServer in dev mode (validator disabled), so the
//! auth middleware is OFF. We exercise the store directly via the REST
//! endpoints, plus a couple of in-process state-machine tests for the
//! bootstrap-platform-admin race-safety guarantee.

mod common;

use serde_json::{Value, json};

#[tokio::test]
async fn org_member_crud() {
    let server = common::TestServer::spawn().await;

    // Direct Account creation isn't exposed via REST yet — the only path
    // is OIDC lazy-create. For the member-CRUD test we drive it via raft
    // by reading the state machine through the cplane's apply path: hit
    // the (auth-disabled) endpoints with a fake account_id pre-seeded.
    //
    // Easier: list the empty members, then assert that grant returns 404
    // because the account_id doesn't exist. That covers the validation
    // path even without a way to create Accounts via REST yet.
    let r = server.get("/orgs/test/members").await;
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert!(body["memberships"].as_array().unwrap().is_empty());

    // Grant to a non-existent account → 404.
    let r = server
        .post_json(
            "/orgs/test/members",
            &json!({"accountId": "acc_nope", "role": "org-admin"}),
        )
        .await;
    assert_eq!(r.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn org_member_grant_unknown_role_400() {
    let server = common::TestServer::spawn().await;
    let r = server
        .post_json(
            "/orgs/test/members",
            &json!({"accountId": "acc_anything", "role": "wizard"}),
        )
        .await;
    assert_eq!(r.status(), 400);

    server.shutdown().await;
}

#[tokio::test]
async fn list_members_404_for_unknown_org() {
    let server = common::TestServer::spawn().await;
    let r = server.get("/orgs/no-such-org/members").await;
    assert_eq!(r.status(), 404);

    server.shutdown().await;
}

#[tokio::test]
async fn me_endpoint_401_without_auth_context() {
    // /v1/me requires AuthContext. In dev (validator None) the middleware
    // doesn't run → no AuthContext attached → handler should 401.
    let server = common::TestServer::spawn().await;
    let r = server.get("/me").await;
    assert_eq!(r.status(), 401);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// State-machine tests: race-safety + idempotency of the apply path.
// ---------------------------------------------------------------------------

mod state_tests {
    use mraft::StateMachine;
    use mvirt_cplane::ApiState;
    use mvirt_cplane::command::{Command, MembershipScope, Response, Role};

    fn fresh_state() -> ApiState {
        ApiState::default()
    }

    fn ensure_account(state: &mut ApiState, sub: &str) -> String {
        let (resp, _) = state.apply(Command::EnsureAccountFromOidc {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            new_id: format!("acc_{}", sub),
            iss: "https://idp.example".into(),
            sub: sub.into(),
            email: Some(format!("{sub}@example")),
            display_name: None,
        });
        match resp {
            Response::Account(a) => a.id,
            other => panic!("expected Account, got {other:?}"),
        }
    }

    #[test]
    fn ensure_account_is_idempotent() {
        let mut s = fresh_state();
        let a1 = ensure_account(&mut s, "u1");
        let a2 = ensure_account(&mut s, "u1");
        assert_eq!(a1, a2, "second EnsureAccount must return same id");
        assert_eq!(s.list_accounts().len(), 1);
    }

    #[test]
    fn bootstrap_platform_admin_is_race_safe() {
        // Two parallel BootstrapInitialPlatformAdmin commands must produce
        // at most one platform-admin membership. The apply handler is the
        // serialization point.
        let mut s = fresh_state();
        let a = ensure_account(&mut s, "first");
        let b = ensure_account(&mut s, "second");
        // Apply both bootstraps; second should be a noop.
        let (_, _) = s.apply(Command::BootstrapInitialPlatformAdmin {
            request_id: "r1".into(),
            timestamp: "t".into(),
            id: "m1".into(),
            account_id: a.clone(),
        });
        let (resp, _) = s.apply(Command::BootstrapInitialPlatformAdmin {
            request_id: "r2".into(),
            timestamp: "t".into(),
            id: "m2".into(),
            account_id: b,
        });
        // The second call returns Ack (idempotent noop), not a fresh membership.
        assert!(matches!(resp, Response::Ack), "second bootstrap must noop");
        let admins: Vec<_> = s
            .list_memberships()
            .into_iter()
            .filter(|m| m.scope == MembershipScope::Platform && m.role == Role::PlatformAdmin)
            .collect();
        assert_eq!(admins.len(), 1, "exactly one platform-admin");
        assert_eq!(admins[0].account_id, a, "first apply wins");
    }

    #[test]
    fn create_membership_rejects_duplicates() {
        let mut s = fresh_state();
        let a = ensure_account(&mut s, "u1");
        s.apply(Command::CreateOrg {
            request_id: "r0".into(),
            timestamp: "t".into(),
            slug: "alpha".into(),
            name: "Alpha".into(),
            default_static_key_ttl_days: 90,
            disallow_static_keys: false,
            contact: Default::default(),
        });
        let (r1, _) = s.apply(Command::CreateMembership {
            request_id: "r1".into(),
            timestamp: "t".into(),
            id: "m1".into(),
            account_id: a.clone(),
            scope: MembershipScope::Org {
                org_slug: "alpha".into(),
            },
            role: Role::OrgAdmin,
            created_by_account: a.clone(),
        });
        assert!(matches!(r1, Response::Membership(_)));
        let (r2, _) = s.apply(Command::CreateMembership {
            request_id: "r2".into(),
            timestamp: "t".into(),
            id: "m2".into(),
            account_id: a,
            scope: MembershipScope::Org {
                org_slug: "alpha".into(),
            },
            role: Role::OrgAdmin,
            created_by_account: "x".into(),
        });
        assert!(
            matches!(r2, Response::Error { code: 409, .. }),
            "second grant must conflict, got {r2:?}"
        );
    }

    #[test]
    fn create_membership_rejects_unknown_scope_target() {
        let mut s = fresh_state();
        let a = ensure_account(&mut s, "u1");
        let (r, _) = s.apply(Command::CreateMembership {
            request_id: "r".into(),
            timestamp: "t".into(),
            id: "m".into(),
            account_id: a,
            scope: MembershipScope::Org {
                org_slug: "ghost".into(),
            },
            role: Role::OrgAdmin,
            created_by_account: "x".into(),
        });
        assert!(matches!(r, Response::Error { code: 404, .. }));
    }
}
