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
async fn project_member_crud() {
    let server = common::TestServer::spawn().await;
    server
        .post_json(
            "/orgs/test/projects",
            &json!({"slug": "demo", "name": "Demo"}),
        )
        .await;

    // Empty initial list.
    let r = server.get("/projects/demo/members").await;
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert!(body["memberships"].as_array().unwrap().is_empty());

    // Grant to nonexistent account → 404.
    let r = server
        .post_json(
            "/projects/demo/members",
            &json!({"accountId": "acc_nope", "role": "project-admin"}),
        )
        .await;
    assert_eq!(r.status(), 404);

    // Bad role → 400.
    let r = server
        .post_json(
            "/projects/demo/members",
            &json!({"accountId": "acc_x", "role": "wizard"}),
        )
        .await;
    assert_eq!(r.status(), 400);

    server.shutdown().await;
}

#[tokio::test]
async fn list_project_members_404_for_unknown_project() {
    let server = common::TestServer::spawn().await;
    let r = server.get("/projects/ghost/members").await;
    assert_eq!(r.status(), 404);
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
async fn cascade_org_admin_via_state_machine() {
    // The is_org_admin / is_platform_admin helpers don't need REST to be
    // exercised; build an AuthContext in-process and assert the cascades.
    use mvirt_cplane::auth::AuthContext;
    use mvirt_cplane::command::{AccountData, AccountKind, MembershipData, MembershipScope, Role};
    let now = "2026-01-01T00:00:00Z".to_string();
    let acc = AccountData {
        id: "acc_x".into(),
        kind: AccountKind::User,
        external_iss: Some("idp".into()),
        external_sub: Some("u".into()),
        email: None,
        display_name: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    // org-admin in "alpha"
    let m_org = MembershipData {
        id: "m1".into(),
        account_id: acc.id.clone(),
        scope: MembershipScope::Org {
            org_slug: "alpha".into(),
        },
        role: Role::OrgAdmin,
        created_by_account: acc.id.clone(),
        created_at: now.clone(),
    };
    let ctx = AuthContext {
        claims: mvirt_cplane::AuthClaims {
            sub: "u".into(),
            iss: "idp".into(),
            exp: 0,
            iat: None,
            email: None,
            name: None,
            given_name: None,
            family_name: None,
            preferred_username: None,
        },
        account: acc.clone(),
        memberships: vec![m_org],
    };
    assert!(ctx.is_org_admin("alpha"), "direct org-admin grant");
    assert!(!ctx.is_org_admin("beta"), "no membership in other org");
    assert!(!ctx.is_platform_admin(), "org-admin is not platform-admin");
    // Project under "alpha" — org-admin cascade applies.
    assert!(ctx.is_project_admin("any-project", Some("alpha")));
    assert!(!ctx.is_project_admin("any-project", Some("beta")));
    assert!(
        !ctx.is_project_admin("any-project", None),
        "no cascade without parent org context"
    );

    // Platform-admin cascades everywhere.
    let plat = AuthContext {
        claims: ctx.claims.clone(),
        account: acc,
        memberships: vec![MembershipData {
            id: "m2".into(),
            account_id: "acc_x".into(),
            scope: MembershipScope::Platform,
            role: Role::PlatformAdmin,
            created_by_account: "acc_x".into(),
            created_at: now,
        }],
    };
    assert!(plat.is_platform_admin());
    assert!(plat.is_org_admin("any"));
    assert!(plat.is_project_admin("any", None));
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
    fn invite_by_email_then_first_oidc_login_links_account() {
        // Operator pre-creates an Account by email. The user later logs in
        // via OIDC with the same email; the apply must link `(iss, sub)`
        // to the existing row rather than minting a duplicate.
        let mut s = fresh_state();
        let (resp, _) = s.apply(Command::CreateAccountByEmail {
            request_id: "r0".into(),
            timestamp: "t".into(),
            id: "acc_invited".into(),
            email: "Malte@example.com".into(), // mixed case → normalised
            display_name: Some("Malte".into()),
        });
        let invited = match resp {
            Response::Account(a) => a,
            other => panic!("expected Account, got {other:?}"),
        };
        assert_eq!(invited.id, "acc_invited");
        assert!(invited.external_iss.is_none(), "no iss yet — invite state");

        // First OIDC login from this email.
        let (resp, _) = s.apply(Command::EnsureAccountFromOidc {
            request_id: "r1".into(),
            timestamp: "t".into(),
            new_id: "acc_should_not_be_used".into(),
            iss: "https://idp.example".into(),
            sub: "sub-123".into(),
            email: Some("malte@example.com".into()),
            display_name: None,
        });
        let linked = match resp {
            Response::Account(a) => a,
            other => panic!("expected Account, got {other:?}"),
        };
        assert_eq!(linked.id, "acc_invited", "linked the existing invite row");
        assert_eq!(linked.external_sub.as_deref(), Some("sub-123"));
        assert_eq!(s.list_accounts().len(), 1, "no duplicate row");

        // Subsequent logins take the (iss, sub) fast path and stay stable.
        let (resp, _) = s.apply(Command::EnsureAccountFromOidc {
            request_id: "r2".into(),
            timestamp: "t".into(),
            new_id: "acc_unused".into(),
            iss: "https://idp.example".into(),
            sub: "sub-123".into(),
            email: Some("malte@example.com".into()),
            display_name: None,
        });
        match resp {
            Response::Account(a) => assert_eq!(a.id, "acc_invited"),
            other => panic!("expected Account, got {other:?}"),
        };
        assert_eq!(s.list_accounts().len(), 1);
    }

    #[test]
    fn invite_by_email_rejects_duplicate_email() {
        let mut s = fresh_state();
        let (r1, _) = s.apply(Command::CreateAccountByEmail {
            request_id: "r1".into(),
            timestamp: "t".into(),
            id: "a1".into(),
            email: "x@example".into(),
            display_name: None,
        });
        assert!(matches!(r1, Response::Account(_)));
        let (r2, _) = s.apply(Command::CreateAccountByEmail {
            request_id: "r2".into(),
            timestamp: "t".into(),
            id: "a2".into(),
            email: "X@Example".into(), // case-insensitive
            display_name: None,
        });
        assert!(matches!(r2, Response::Error { code: 409, .. }));
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
