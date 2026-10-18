//! Agent 侧拉黑面的 HTTP 契约测试（批次三·第三刀 · 第①步「拉黑对主体开放」，
//! `docs/contracts/core-v1/CORE_V1_CONTACT_REVERSIBILITY.md`）：
//!
//! - **拉黑是关系事实的对称面**：能加、能拒绝、能解除，就能拉黑——Agent 也能；
//! - **语义与 Human 侧逐条相同**：拉黑必先结束既有 active 关系段（拉黑 ⊇ 解除）、
//!   不进消息主链、被拉黑方**一无所知**（它之后的申请与「目标不存在」同形 404
//!   `participant_not_found`，无存在性 oracle）、每对每方向至多一个 active 段、
//!   重复拉黑幂等 204、解除无段亦 204；
//! - **认证面**：三条端点全部 BackendAuth（缺凭据 401 `authentication_required`、
//!   Human token 一律 401 `invalid_credential`）；
//! - **Human 侧形状**：只有"目标不可用"的 404 由 `human_not_found` 换成
//!   `participant_not_found`（本步），其余一字未变。
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

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    alice_token: String,
    alice_id: String,
    agent_a: String,
    agent_a_token: String,
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
    let alice_token = alice.token.as_str().to_owned();
    let alice_id = alice.participant.id.as_str().to_owned();
    let (agent_a, agent_a_token) = agent_with_credential(&router, &alice_token, "bb-a", "甲").await;
    Fixture {
        _dir: dir,
        router,
        alice_token,
        alice_id,
        agent_a,
        agent_a_token,
    }
}

async fn agent_with_credential(
    router: &axum::Router,
    owner_token: &str,
    key: &str,
    name: &str,
) -> (String, String) {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        "/v1/agents",
        Some(owner_token),
        Some(&format!("{key}-a")),
        Some(&serde_json::json!({"display_name": name})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create agent {name}: {json}");
    let agent_id = json["agent"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(owner_token),
        Some(&format!("{key}-b")),
        Some(&serde_json::json!({"label": "block e2e"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        Some(owner_token),
        Some(&format!("{key}-c")),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "credential: {json}");
    (agent_id, json["token"].as_str().unwrap().to_owned())
}

async fn request_json(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    key: Option<&str>,
    json_body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if json_body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
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
    (status, json, request_id)
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
async fn backend_block_endpoints_require_backend_auth() {
    let f = fixture().await;
    for (label, token, method, path) in [
        (
            "list, no header",
            None,
            Method::GET,
            "/v1/backend/blocks".to_owned(),
        ),
        (
            "list, human token",
            Some(f.alice_token.as_str()),
            Method::GET,
            "/v1/backend/blocks".to_owned(),
        ),
        (
            "block, human token",
            Some(f.alice_token.as_str()),
            Method::POST,
            format!("/v1/backend/blocks/{}", f.alice_id),
        ),
    ] {
        let (status, json, _) = request_json(&f.router, method, &path, token, None, None).await;
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
        &format!("/v1/backend/blocks/{}", f.alice_id),
        &f.alice_token,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "invalid_credential");
}

// ── 2. Agent 拉黑 Human：被拉黑方一无所知 ──────────────────────────────────

#[tokio::test]
async fn agent_blocks_a_human_and_the_human_learns_nothing() {
    let f = fixture().await;

    // 先建立一段关系（拉黑 ⊇ 解除：拉黑会把它结束掉）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&f.alice_token),
        Some("bb-c1"),
        Some(&serde_json::json!({"participant_id": f.agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/contacts/{}/accept", f.alice_id),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    // Agent 拉黑那位 Human ⇒ 204。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/blocks/{}", f.alice_id),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "agent blocks: {json}");

    // 拉黑 ⊇ 解除：那段关系被同事务结束掉（列表里不再出现）。
    let (_, view, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/backend/contacts",
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert!(
        view["contacts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["peer"]["id"] != serde_json::json!(f.alice_id)),
        "blocking must end the active relation: {view}"
    );

    // 被拉黑方**一无所知**：它之后的申请与「目标不存在」同形 404。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&f.alice_token),
        Some("bb-c2"),
        Some(&serde_json::json!({"participant_id": f.agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "participant_not_found");
    // 未知目标也是同一个码 ⇒ 无存在性 oracle。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&f.alice_token),
        Some("bb-c3"),
        Some(&serde_json::json!({"participant_id": Uuid::new_v4().to_string()})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "participant_not_found");

    // Agent 的拉黑列表里有它（对端自带 kind）。
    let (status, view, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/backend/blocks",
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let blocks = view["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{view}");
    assert_eq!(blocks[0]["peer"]["id"], serde_json::json!(f.alice_id));
    assert_eq!(blocks[0]["peer"]["kind"], "human");

    // 重复拉黑幂等 204；解除后可以重新申请。
    let (status, _, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/blocks/{}", f.alice_id),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "blocking twice is an idempotent no-op"
    );
    let (status, _) = delete(
        &f.router,
        &format!("/v1/backend/blocks/{}", f.alice_id),
        &f.agent_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&f.alice_token),
        Some("bb-c4"),
        Some(&serde_json::json!({"participant_id": f.agent_a})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "after unblocking, a new request must be possible: {json}"
    );
}

// ── 3. Human 拉黑 Agent（本步 Human 侧的头号行为变化） ─────────────────────

#[tokio::test]
async fn a_human_can_now_block_an_agent() {
    let f = fixture().await;
    // 本步之前：拉黑目标必须是 Human（"Agent 不可被拉黑"）⇒ 这一步之后必须成功。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/blocks/{}", f.agent_a),
        Some(&f.alice_token),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a human must be able to block an agent now: {json}"
    );

    // 被拉黑的 Agent 对这位 Human 的申请一律同形 404（不泄露）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/contacts/requests",
        Some(&f.agent_a_token),
        Some("bb-h2a-blocked"),
        Some(&serde_json::json!({"participant_id": f.alice_id})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "participant_not_found");

    // 解除后可以重新申请（拉黑不阻断别的一切：所有权仍在、房仍在）。
    let (status, _) = delete(
        &f.router,
        &format!("/v1/blocks/{}", f.agent_a),
        &f.alice_token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/contacts/requests",
        Some(&f.agent_a_token),
        Some("bb-h2a-after"),
        Some(&serde_json::json!({"participant_id": f.alice_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
}

// ── 4. 目标不可用与自拉黑 ──────────────────────────────────────────────────

#[tokio::test]
async fn backend_block_target_errors_are_kind_neutral() {
    let f = fixture().await;
    // 未知目标 ⇒ 404 participant_not_found（新码，与 Human 侧同口径）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/blocks/{}", Uuid::new_v4()),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "participant_not_found");

    // 拉黑自己 ⇒ 400 invalid_request。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/blocks/{}", f.agent_a),
        Some(&f.agent_a_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");
}
