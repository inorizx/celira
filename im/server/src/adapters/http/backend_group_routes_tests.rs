//! Agent 侧房治理端点的 HTTP 契约测试（批次三·第一刀「owner = 创建者」，
//! `docs/contracts/core-v1/CORE_V1_IM_CONTRACT.md` §5.1/§5.4/§5.6）：
//!
//! - **owner 是房内的治理角色（role），不是参与者种类（kind）**：一支 Agent 能建房
//!   并成为 owner，能移除任何 active 非 owner 成员（含 Human），能把 owner 转让给
//!   任意 active 成员，也能在转让之后退出自己的房（M2 规则：owner 先转让）；
//! - **认证面**：四条写端点全部 BackendAuth（缺凭据 401 `authentication_required`、
//!   Human token 一律 401 `invalid_credential`）；
//! - **邀请是一条判据**：目标要与邀请者有 accepted 联系人段，或被邀请者以 active
//!   Agent Owner 拥有——不按"邀请者是不是 Human"分叉。Agent owner 今天两条都取不到
//!   （它名下没有那两类关系事实），所以稳定 404 `member_not_found`；
//! - **Human 侧一字不变**：同一批 application 用例与同一段响应构造，只换认证面。
//!
//! 断言全走公共 HTTP 协议，不直读 SQLite。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::identity;
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::identity::{IdentityStore as _, NewSession};

const KEY_HEADER: &str = "idempotency-key";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    human_token: String,
    human_participant_id: String,
    bob_id: String,
    bob_token: String,
    agent_a: String,
    agent_a_token: String,
    agent_b: String,
    agent_b_token: String,
    /// Alice 建、**已把 owner 转让给 Agent A** 的群（A=owner、Alice=member）。
    group_owned_by_agent: String,
    /// Alice(owner) + Bob(member) + Agent B(member) 的群（非 owner 的对照）。
    group_owned_by_alice: String,
}

async fn add_human(
    pool: &sqlx::SqlitePool,
    identity: &SqliteIdentityStore,
    display_name: &str,
) -> (String, String) {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', ?, 0, NULL)",
    )
    .bind(&id)
    .bind(display_name)
    .execute(pool)
    .await
    .expect("insert second human");
    let token = OpaqueToken::generate().expect("token");
    let session = HumanSession::new(
        SessionId(Uuid::new_v4().to_string()),
        ParticipantId(id.clone()),
        TokenHash::from_token(&token),
        UtcMillis::now(),
    );
    identity
        .issue_human_session(&NewSession {
            id: session.id.clone(),
            human_id: session.human_id.clone(),
            token_hash: session.token_hash,
            created_at: session.created_at,
        })
        .await
        .expect("issue session");
    (id, token.as_str().to_owned())
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent = SqliteAgentStore::new(pool.clone());
    let room = SqliteRoomStore::new(pool.clone());
    let alice = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap alice");
    let (bob_id, bob_token) = add_human(&pool, &identity, "Bob").await;
    let router = build_router(
        Arc::new(store),
        Arc::new(identity),
        Arc::new(agent),
        Arc::new(room),
        Arc::new(SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::event_store::SqliteEventStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::contact_store::SqliteContactStore::new(pool.clone())),
        Arc::new(
            crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(pool.clone()),
        ),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(
            crate::adapters::sqlite::attachment_store::SqliteAttachmentStore::new(pool.clone()),
        ),
        Arc::new(crate::adapters::blob::LocalBlobStorage::new(
            dir.path().join("media"),
        )),
        Arc::new(crate::adapters::sqlite::home_store::SqliteHomeStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    let human_token = alice.token.as_str().to_owned();

    let mut agents = Vec::new();
    for (i, name) in [(1, "甲"), (2, "乙")] {
        let (status, json, _) = request_json(
            &router,
            Method::POST,
            "/v1/agents",
            &human_token,
            Some(&format!("bg-a-{i}")),
            Some(&serde_json::json!({"display_name": name})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create agent {name}: {json}");
        agents.push(json["agent"]["id"].as_str().unwrap().to_owned());
    }
    let agent_a = agents[0].clone();
    let agent_b = agents[1].clone();
    let agent_a_token = credential_for(&router, &human_token, &agent_a, "bg-cred-a").await;
    let agent_b_token = credential_for(&router, &human_token, &agent_b, "bg-cred-b").await;

    // Alice ↔ Bob 建立 accepted 联系人（Human 侧邀请资格要用）。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/contacts/requests",
        &human_token,
        Some("bg-cr-1"),
        Some(&serde_json::json!({"participant_id": bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "contact request: {json}");
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", alice.participant.id.as_str()),
        &bob_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "contact accept: {json}");

    // 群 1：Alice 建群 → 拉 Agent A → **把 owner 转让给 A**（Human 侧端点）。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/groups",
        &human_token,
        Some("bg-g-1"),
        Some(&serde_json::json!({"name": "甲当家的群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create group 1: {json}");
    let group_owned_by_agent = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        &format!("/v1/rooms/{group_owned_by_agent}/members"),
        &human_token,
        Some("bg-g-1-i"),
        Some(&serde_json::json!({"participant_id": agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite A: {json}");
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        &format!("/v1/rooms/{group_owned_by_agent}/owner-transfer"),
        &human_token,
        Some("bg-g-1-t"),
        Some(&serde_json::json!({"participant_id": agent_a})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "transfer to an Agent member must succeed (owner 是 role，不是 kind): {json}"
    );
    assert_eq!(json["membership"]["role"], "owner");
    assert_eq!(
        json["membership"]["participant_id"],
        serde_json::json!(agent_a)
    );

    // 群 2：Alice(owner) + Bob(member) + Agent B(member)——非 owner 的对照面。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/groups",
        &human_token,
        Some("bg-g-2"),
        Some(&serde_json::json!({"name": "Alice 当家的群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create group 2: {json}");
    let group_owned_by_alice = json["room"]["id"].as_str().unwrap().to_owned();
    for (key, target) in [("bg-g-2-b", bob_id.clone()), ("bg-g-2-a", agent_b.clone())] {
        let (status, json, _) = request_json(
            &router,
            Method::POST,
            &format!("/v1/rooms/{group_owned_by_alice}/members"),
            &human_token,
            Some(key),
            Some(&serde_json::json!({"participant_id": target})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "invite {target}: {json}");
    }

    Fixture {
        _dir: dir,
        router,
        human_token,
        human_participant_id: alice.participant.id.as_str().to_owned(),
        bob_id,
        bob_token,
        agent_a,
        agent_a_token,
        agent_b,
        agent_b_token,
        group_owned_by_agent,
        group_owned_by_alice,
    }
}

async fn credential_for(
    router: &axum::Router,
    human_token: &str,
    agent_id: &str,
    key: &str,
) -> String {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        human_token,
        Some(&format!("{key}-b")),
        Some(&serde_json::json!({"label": "group admin e2e"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        human_token,
        Some(&format!("{key}-c")),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "issue credential: {json}");
    json["token"].as_str().unwrap().to_owned()
}

async fn request_json(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: &str,
    key: Option<&str>,
    json_body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let (status, json, request_id, _) =
        request_json_maybe_anonymous(router, method, path, Some(token), key, json_body).await;
    (status, json, request_id)
}

/// 与 `request_json` 同，但 Bearer 可空（判 401 的两种形态）。
async fn request_json_maybe_anonymous(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    key: Option<&str>,
    json_body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value, Option<String>, ()) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if json_body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(key) = key {
        builder = builder.header(KEY_HEADER, key);
    }
    let response = router
        .clone()
        .oneshot(
            builder
                .body(Body::from(
                    json_body
                        .map(|v| v.to_string().into_bytes())
                        .unwrap_or_default(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json, request_id, ())
}

async fn delete(router: &axum::Router, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json)
}

// ── 1. 认证面 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn backend_group_writes_require_backend_auth() {
    let f = fixture().await;
    for (label, token, method, path) in [
        (
            "create group, no header",
            None,
            Method::POST,
            "/v1/backend/groups",
        ),
        (
            "create group, human token",
            Some(f.human_token.as_str()),
            Method::POST,
            "/v1/backend/groups",
        ),
        (
            "invite, human token",
            Some(f.human_token.as_str()),
            Method::POST,
            &format!("/v1/backend/rooms/{}/members", f.group_owned_by_alice),
        ),
        (
            "transfer, human token",
            Some(f.human_token.as_str()),
            Method::POST,
            &format!(
                "/v1/backend/rooms/{}/owner-transfer",
                f.group_owned_by_alice
            ),
        ),
    ] {
        let body = serde_json::json!({"name": "x", "participant_id": f.bob_id});
        let (status, json, _, _) = request_json_maybe_anonymous(
            &f.router,
            method,
            path,
            token,
            Some("bg-auth-1"),
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}: {json}");
        let expected = if token.is_none() {
            "authentication_required"
        } else {
            "invalid_credential"
        };
        assert_eq!(json["error"]["code"], expected, "{label}");
    }
    let (status, json) = delete(
        &f.router,
        &format!(
            "/v1/backend/rooms/{}/members/{}",
            f.group_owned_by_alice, f.bob_id
        ),
        &f.human_token,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "invalid_credential");
}

// ── 2. Agent 建房 ⇒ 它自己是 owner ─────────────────────────────────────────

#[tokio::test]
async fn agent_creates_group_and_is_its_owner() {
    let f = fixture().await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/groups",
        &f.agent_a_token,
        Some("bg-new-group"),
        Some(&serde_json::json!({"name": "甲乙的房间"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent creates group: {json}");
    let room_id = json["room"]["id"].as_str().unwrap().to_owned();
    assert_eq!(json["room"]["kind"], "group");
    assert_eq!(json["room"]["name"], "甲乙的房间");
    let memberships = json["memberships"].as_array().unwrap();
    assert_eq!(memberships.len(), 1, "only the founding owner: {json}");
    assert_eq!(
        memberships[0]["participant_id"],
        serde_json::json!(f.agent_a)
    );
    assert_eq!(memberships[0]["role"], "owner");
    assert_eq!(json["idempotency_replayed"], false);

    // 同 key 同群名 ⇒ 200 重放同一间房；异群名 ⇒ 409。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/groups",
        &f.agent_a_token,
        Some("bg-new-group"),
        Some(&serde_json::json!({"name": "甲乙的房间"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replay: {json}");
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["room"]["id"], serde_json::json!(room_id));

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/groups",
        &f.agent_a_token,
        Some("bg-new-group"),
        Some(&serde_json::json!({"name": "另一个名字"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "same key, other name: {json}");
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // 通讯录（Agent 读面）里看得见这间房，且它是 owner 的那个成员周期。
    let (status, json, _, _) = request_json_maybe_anonymous(
        &f.router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "directory: {json}");
    let hit = json["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["room_id"] == serde_json::json!(room_id))
        .expect("the new group must be in the agent's directory");
    assert_eq!(hit["kind"], "group");
}

// ── 3. Agent owner 的成员治理（移除 / 不能移除自己 / 转让 / 退出）───────────

#[tokio::test]
async fn agent_owner_removes_a_human_member() {
    let f = fixture().await;
    let (status, json) = delete(
        &f.router,
        &format!(
            "/v1/backend/rooms/{}/members/{}",
            f.group_owned_by_agent,
            f.human_id().to_owned()
        ),
        &f.agent_a_token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "an Agent owner may remove a Human member: {json}"
    );

    // 被移除的 Human 从此对该房 404（窗口关闭，历史保留在服务端）。
    let (status, json, _, _) = request_json_maybe_anonymous(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{}", f.group_owned_by_agent),
        Some(&f.human_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "removed human: {json}");
    assert_eq!(json["error"]["code"], "room_not_found");
}

#[tokio::test]
async fn agent_owner_cannot_remove_itself() {
    let f = fixture().await;
    let (status, json) = delete(
        &f.router,
        &format!(
            "/v1/backend/rooms/{}/members/{}",
            f.group_owned_by_agent, f.agent_a
        ),
        &f.agent_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "owner_not_removable");
}

#[tokio::test]
async fn agent_owner_transfers_ownership_and_then_leaves() {
    let f = fixture().await;

    // owner 直接退房 ⇒ 409（M2 的既有规则，不分 kind）。
    let (status, json, _, _) = request_json_maybe_anonymous(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{}/leave", f.group_owned_by_agent),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "owner leave: {json}");
    assert_eq!(json["error"]["code"], "owner_transfer_required");

    // 转让给那位 Human 成员（Agent actor 发起，BackendAuth）。
    let human_id = f.human_id().to_owned();
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!(
            "/v1/backend/rooms/{}/owner-transfer",
            f.group_owned_by_agent
        ),
        &f.agent_a_token,
        Some("bg-a-transfer-back"),
        Some(&serde_json::json!({"participant_id": human_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agent transfers back: {json}");
    assert_eq!(json["membership"]["role"], "owner");
    assert_eq!(
        json["membership"]["participant_id"],
        serde_json::json!(human_id)
    );

    // 转让之后它不再是 owner ⇒ 可以自己退房（204）——"不会被困在自己的房里"。
    let (status, json, _, _) = request_json_maybe_anonymous(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{}/leave", f.group_owned_by_agent),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "agent leaves: {json}");
}

// ── 4. 邀请是一条判据（不分"邀请者是不是 Human"）───────────────────────────

#[tokio::test]
async fn invite_qualification_is_one_rule_not_two() {
    let f = fixture().await;
    // Agent A 是 owner：它今天两条关系都没有 ⇒ 两类目标都统一 404 member_not_found。
    for (label, target) in [
        ("a Human it has no accepted contact with", f.bob_id.clone()),
        ("an Agent it does not own", f.agent_b.clone()),
    ] {
        let (status, json, _) = request_json(
            &f.router,
            Method::POST,
            &format!("/v1/backend/rooms/{}/members", f.group_owned_by_agent),
            &f.agent_a_token,
            Some(&format!("bg-inv-{}", target)),
            Some(&serde_json::json!({"participant_id": target})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label}: {json}");
        assert_eq!(json["error"]["code"], "member_not_found", "{label}");
    }
}

// ── 5. 非 owner 的治理动作一律 404（不分 kind）─────────────────────────────

#[tokio::test]
async fn non_owner_agent_gets_room_not_found() {
    let f = fixture().await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{}/members", f.group_owned_by_alice),
        &f.agent_b_token,
        Some("bg-nonowner-inv"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "invite: {json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    let (status, json) = delete(
        &f.router,
        &format!(
            "/v1/backend/rooms/{}/members/{}",
            f.group_owned_by_alice, f.bob_id
        ),
        &f.agent_b_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "remove: {json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!(
            "/v1/backend/rooms/{}/owner-transfer",
            f.group_owned_by_alice
        ),
        &f.agent_b_token,
        Some("bg-nonowner-t"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "transfer: {json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // 对照（Human 侧一字不变）：Bob 是那个群的 active member，读得到它。
    let (status, json, _, _) = request_json_maybe_anonymous(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{}", f.group_owned_by_alice),
        Some(&f.bob_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "human member still reads: {json}");
    assert_eq!(
        json["room"]["id"],
        serde_json::json!(f.group_owned_by_alice)
    );
}

// ── 6. 转让目标的判据：不是本房成员 ⇒ 404（不分 kind）──────────────────────

#[tokio::test]
async fn transfer_to_a_non_member_agent_is_member_not_found() {
    let f = fixture().await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/owner-transfer", f.group_owned_by_alice),
        &f.human_token,
        Some("bg-t-nonmember-agent"),
        Some(&serde_json::json!({"participant_id": f.agent_a})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an Agent outside the room is simply not a member: {json}"
    );
    assert_eq!(json["error"]["code"], "member_not_found");

    // 转让给自己 ⇒ 400 invalid_transfer_target（这一条一字未变）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/owner-transfer", f.group_owned_by_alice),
        &f.human_token,
        Some("bg-t-self"),
        Some(&serde_json::json!({"participant_id": f.human_id().to_owned()})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_transfer_target");
}

impl Fixture {
    /// Alice（主人）的 participant id。
    fn human_id(&self) -> &str {
        &self.human_participant_id
    }
}
