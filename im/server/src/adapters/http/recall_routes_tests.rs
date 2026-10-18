//! Message Recall HTTP 契约测试（CORE_V1_MESSAGE_RECALL.md §12）：双入口
//! 认证适配（Human Session / Backend Credential）、请求形状约束（无 body /
//! 无 query / 无 Idempotency-Key）、201/200 幂等、错误优先与脱敏、tombstone
//! 公共投影（History / 发送重放 / Backend Event）、Human Event 收敛、以及
//! Completion / reply 相邻契约的 HTTP 面。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::{Connection, SqlitePool};
use tower::ServiceExt;
use uuid::Uuid;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::human_event_store::SqliteHumanEventStore;
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
const AUTH_JSON: &str = "application/json";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    token_a: String,
    human_a: String,
    token_b: String,
    pool: SqlitePool,
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session。
async fn add_human(
    pool: &SqlitePool,
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
        .expect("issue session for second human");
    (id, token.as_str().to_owned())
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent = SqliteAgentStore::new(pool.clone());
    let room = SqliteRoomStore::new(pool.clone());
    let a = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap alice");
    let (_human_b, token_b) = add_human(&pool, &identity, "Bob").await;
    let router = build_router(
        Arc::new(store),
        Arc::new(identity.clone()),
        Arc::new(agent),
        Arc::new(room),
        Arc::new(SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(SqliteEventStore::new(pool.clone())),
        Arc::new(SqliteContactStore::new(pool.clone())),
        Arc::new(SqliteHumanEventStore::new(pool.clone())),
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
    Fixture {
        _dir: dir,
        router,
        token_a: a.token.as_str().to_owned(),
        human_a: a.participant.id.as_str().to_owned(),
        token_b,
        pool,
    }
}

async fn send(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.unwrap_or_default())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json, request_id)
}

/// 通过公开 API 创建 Agent，返回 Agent ID。
async fn create_agent(router: &axum::Router, token: &str, display_name: &str, key: &str) -> String {
    let body = serde_json::json!({ "display_name": display_name }).to_string();
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent must be created");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

/// 通过公开 API 创建 Direct Room，返回 Room ID。
async fn create_room(router: &axum::Router, token: &str, agent_id: &str, key: &str) -> String {
    let body = serde_json::json!({ "agent_id": agent_id }).to_string();
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "room must be created");
    json["room"]["id"].as_str().unwrap().to_owned()
}

/// 通过公开 API 发送普通 Message，返回 Message ID。
async fn send_message(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    body: &str,
    key: &str,
) -> String {
    let body = serde_json::json!({ "body": body }).to_string();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "message must be created: {json}"
    );
    json["message"]["id"].as_str().unwrap().to_owned()
}

/// 创建 Backend Binding + Credential，返回 (credential_id, token)。
async fn issue_backend_token(
    router: &axum::Router,
    human_token: &str,
    agent_id: &str,
) -> (String, String) {
    let body = serde_json::json!({ "label": "local runtime" }).to_string();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(&format!("Bearer {human_token}")),
        &[(KEY_HEADER, "rb-1"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let body = serde_json::json!({ "replaces_credential_id": null }).to_string();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        Some(&format!("Bearer {human_token}")),
        &[(KEY_HEADER, "rc-1"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "credential: {json}");
    let credential_id = json["credential"]["id"].as_str().unwrap().to_owned();
    let token = json["token"].as_str().unwrap().to_owned();
    (credential_id, token)
}

/// Backend 提交 reply Completion，返回 reply Message ID。
/// Agent 以成员身份在房里说一句（返回新 Message id）。
/// ⚠ 2026-09-28：原为 `submit_reply`（Completion reply 写回）；Completion/ACK
/// 整体退役后改走 say——同一 message 用例、同一幂等面。
async fn say(
    router: &axum::Router,
    backend_token: &str,
    room_id: &str,
    body: &str,
    key: &str,
) -> String {
    let payload = serde_json::json!({ "body": body }).to_string();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/backend/rooms/{room_id}/messages"),
        Some(&format!("Bearer {backend_token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(payload.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "say: {json}");
    json["message"]["id"].as_str().unwrap().to_owned()
}

/// Human 撤回辅助。
async fn recall_human(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    message_id: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        router,
        Method::DELETE,
        &format!("/v1/rooms/{room_id}/messages/{message_id}"),
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await
}

/// 直接 SQL 构建 backdated group 房间（窗口过期测试用），返回 Room ID。
async fn backdated_group_room(pool: &SqlitePool, owner_id: &str) -> String {
    let room_id = Uuid::new_v4().to_string();
    let membership_id = Uuid::new_v4().to_string();
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at, name) \
         VALUES (?, 'group', 'active', ?, 'fixture')",
    )
    .bind(&room_id)
    .bind(-400_000)
    .execute(&mut *tx)
    .await
    .expect("fixture room");
    sqlx::query(
        "INSERT INTO room_coverage (room_id, kind, created_at, group_owner_membership_id) \
         VALUES (?, 'group', ?, ?)",
    )
    .bind(&room_id)
    .bind(-400_000)
    .bind(&membership_id)
    .execute(&mut *tx)
    .await
    .expect("fixture coverage");
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence) \
         VALUES (?, ?, ?, 'owner', ?, 0)",
    )
    .bind(&membership_id)
    .bind(&room_id)
    .bind(owner_id)
    .bind(-400_000)
    .execute(&mut *tx)
    .await
    .expect("fixture membership");
    tx.commit().await.expect("fixture commit");
    room_id
}

/// 直接 SQL 插入 created_at = statement-now + offset 的 Message，返回 ID。
async fn insert_message_at_offset(
    pool: &SqlitePool,
    room_id: &str,
    actor_id: &str,
    body: &str,
    key: &str,
    offset_ms: i64,
) -> String {
    let id = Uuid::new_v4().to_string();
    sqlx::query(&format!(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE room_id = ?), \
                 ?, ?, NULL, ?, \
                 CAST(unixepoch('subsec') * 1000 AS INTEGER) + {offset_ms})"
    ))
    .bind(&id)
    .bind(room_id)
    .bind(room_id)
    .bind(actor_id)
    .bind(body)
    .bind(key)
    .execute(pool)
    .await
    .expect("raw message fixture");
    id
}

// ─────────────────────────── Human 入口主路径 ───────────────────────────

#[tokio::test]
async fn human_recall_first_201_replay_200_with_same_recalled_at() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;

    let (status, json, request_id) = recall_human(&f.router, &f.token_a, &room_id, &msg_id).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "first recall must be 201: {json}"
    );
    assert_eq!(json["message_id"], msg_id);
    assert_eq!(json["room_id"], room_id);
    assert_eq!(json["idempotency_replayed"], false);
    let recalled_at = json["recalled_at"]
        .as_str()
        .expect("recalled_at string")
        .to_owned();
    // RFC3339 时间戳；近似当前时刻。
    let parsed = chrono_like_parse(&recalled_at);
    assert!(
        parsed > 0,
        "recalled_at must be a real timestamp: {recalled_at}"
    );
    assert_eq!(json["request_id"], request_id.as_deref().unwrap_or(""));
    // 响应不含内部字段。
    let text = serde_json::to_string(&json).unwrap();
    for leak in ["sequence", "source_id", "advancement", "body"] {
        assert!(
            !text.contains(leak),
            "response must not leak {leak:?}: {text}"
        );
    }

    // 重复同一撤回：200 幂等重放、recalled_at 不刷新。
    let (status, json, request_id2) = recall_human(&f.router, &f.token_a, &room_id, &msg_id).await;
    assert_eq!(status, StatusCode::OK, "replay must be 200: {json}");
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(
        json["recalled_at"], recalled_at,
        "recalled_at must not refresh"
    );
    assert!(request_id2.is_some());
}

/// RFC3339 时间解析近似校验（返回 epoch 毫秒；非法返回 0）。
fn chrono_like_parse(rfc3339: &str) -> i64 {
    let bytes = rfc3339.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[10] != b'T' {
        return 0;
    }
    let year: i64 = rfc3339[0..4].parse().unwrap_or(0);
    let month: i64 = rfc3339[5..7].parse().unwrap_or(0);
    let day: i64 = rfc3339[8..10].parse().unwrap_or(0);
    let hour: i64 = rfc3339[11..13].parse().unwrap_or(0);
    let minute: i64 = rfc3339[14..16].parse().unwrap_or(0);
    let second: i64 = rfc3339[17..19].parse().unwrap_or(0);
    let epoch =
        year * 31_536_000 + month * 2_592_000 + day * 86_400 + hour * 3600 + minute * 60 + second;
    epoch * 1000
}

// ─────────────────────────── 认证与错误矩阵 ───────────────────────────

#[tokio::test]
async fn human_recall_auth_errors_and_wrong_token_boundaries() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;

    // 无认证 → 401 authentication_required。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{room_id}/messages/{msg_id}"),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");

    // 坏 Session → 401 invalid_session（Human 入口语义）。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{room_id}/messages/{msg_id}"),
        Some("Bearer clr_hs_not-a-real-token"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");

    // 非成员 Human（Bob）→ 404 room_not_found（先于消息裁决）。
    let (status, json, _) = recall_human(&f.router, &f.token_b, &room_id, &msg_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");

    // bad room id → 404 room_not_found。
    let (status, json, _) = recall_human(&f.router, &f.token_a, "not-a-room", &msg_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");

    // bad message UUID → 400 invalid_request。
    let (status, json, _) = recall_human(&f.router, &f.token_a, &room_id, "not-a-uuid").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    // unknown message → 404 message_not_found。
    let unknown = Uuid::new_v4().to_string();
    let (status, json, _) = recall_human(&f.router, &f.token_a, &room_id, &unknown).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "message_not_found");

    // 跨 room → 404 message_not_found。
    let room_b = create_room(
        &f.router,
        &f.token_a,
        &create_agent(&f.router, &f.token_a, "二号", "a-2").await,
        "r-2",
    )
    .await;
    let (status, json, _) = recall_human(&f.router, &f.token_a, &room_b, &msg_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "message_not_found");

    // 错误信封：error.request_id 与 x-request-id 一致，无 SQL/路径/token 泄露。
    let (_, json, request_id) = recall_human(&f.router, &f.token_a, &room_id, &unknown).await;
    assert_eq!(
        json["error"]["request_id"],
        request_id.as_deref().unwrap_or(""),
        "error envelope must carry the canonical request_id"
    );
    let text = serde_json::to_string(&json).unwrap();
    for leak in ["sql", "database", "core.db", "token", "clr_hs", "hello"] {
        assert!(
            !text.to_lowercase().contains(&leak.to_lowercase()),
            "must not leak: {text}"
        );
    }
}

#[tokio::test]
async fn human_cannot_recall_agent_message() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    let (_, backend_token) = issue_backend_token(&f.router, &f.token_a, &agent_id).await;
    let _ = &msg_id;
    // ⚠ 2026-09-28：Agent 的那条消息原由 Completion reply 写回；退役后走 say。
    let reply_id = say(&f.router, &backend_token, &room_id, "echo: hello", "rk-1").await;

    // Human 撤回 Agent 的 reply Message → 404 message_not_found（统一、不
    // 泄露归属），无副作用。
    let (status, json, _) = recall_human(&f.router, &f.token_a, &room_id, &reply_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "message_not_found");
    let recalls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_recalls")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(recalls, 0);
}

// ─────────────────────────── Backend 入口 ───────────────────────────

#[tokio::test]
async fn backend_recall_of_own_reply_201_and_auth_boundaries() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    let (_, backend_token) = issue_backend_token(&f.router, &f.token_a, &agent_id).await;
    let _ = &msg_id;
    // ⚠ 2026-09-28：Agent 那条消息原由 Completion reply 写回；退役后走 say。
    let reply_id = say(&f.router, &backend_token, &room_id, "echo: hello", "rk-1").await;

    // Agent 经 Backend 入口撤回自己的 reply Message → 201。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/backend/rooms/{room_id}/messages/{reply_id}"),
        Some(&format!("Bearer {backend_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "agent recall must be 201: {json}"
    );
    assert_eq!(json["message_id"], reply_id);
    assert_eq!(json["room_id"], room_id);
    assert_eq!(json["idempotency_replayed"], false);

    // 重复 → 200 幂等重放。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/backend/rooms/{room_id}/messages/{reply_id}"),
        Some(&format!("Bearer {backend_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);

    // Backend 入口接受 Human Session token → 401 invalid_credential。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/backend/rooms/{room_id}/messages/{reply_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_credential");

    // Human 入口接受 Backend Credential token → 401 invalid_session。
    let (status, json, _) = recall_human(&f.router, &backend_token, &room_id, &reply_id).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");

    // Backend 撤回 Human 的 Message（非本人）→ 404 message_not_found。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/backend/rooms/{room_id}/messages/{msg_id}"),
        Some(&format!("Bearer {backend_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "message_not_found");

    // Agent 回复不生成 Agent Recall Event（无 created target），但回复消息
    // 撤回后 Human Event 正常收敛。
    let recalled_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_events WHERE kind = 'message.recalled'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        recalled_events, 0,
        "agent reply has no created event, so no recall event"
    );
    let recalled_facts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'message.recalled'",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(recalled_facts, 1);
}

#[tokio::test]
async fn backend_recall_requires_no_body_query_or_idempotency_key() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    let (_, backend_token) = issue_backend_token(&f.router, &f.token_a, &agent_id).await;

    // 形状约束：非空 body / 任意 query / Idempotency-Key 一律 400
    // invalid_request（不接受、不静默忽略）。
    let path = format!("/v1/backend/rooms/{room_id}/messages/{msg_id}");
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &path,
        Some(&format!("Bearer {backend_token}")),
        &[("content-type", AUTH_JSON)],
        Some(b"{}".to_vec()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "body must be rejected: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_request");

    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("{path}?x=1"),
        Some(&format!("Bearer {backend_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "query must be rejected: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_request");

    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &path,
        Some(&format!("Bearer {backend_token}")),
        &[(KEY_HEADER, "any-key")],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Idempotency-Key must be rejected: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_request");

    // 形状失败的撤回不产生任何事实。
    let recalls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_recalls")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(recalls, 0);
}

// ─────────────────────────── 窗口与公共投影 ───────────────────────────

#[tokio::test]
async fn human_recall_window_expired_is_409_and_no_facts() {
    let f = fixture().await;
    let stale_room = backdated_group_room(&f.pool, &f.human_a).await;
    let stale_id =
        insert_message_at_offset(&f.pool, &stale_room, &f.human_a, "stale", "ws-1", -130_000).await;

    let (status, json, _) = recall_human(&f.router, &f.token_a, &stale_room, &stale_id).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "expired recall must be 409: {json}"
    );
    assert_eq!(json["error"]["code"], "message_recall_window_expired");
    let recalls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_recalls")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(recalls, 0, "expired recall must leave no facts");
}

#[tokio::test]
async fn history_send_replay_and_events_show_tombstone_without_original_body() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "secret-original", "m-1").await;
    let (_, backend_token) = issue_backend_token(&f.router, &f.token_a, &agent_id).await;

    // 发送响应（active）：字段恒存在——body 原值、mentions=[]、
    // recalled_at=null；无 sequence。
    let body = serde_json::json!({ "body": "secret-original" }).to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "m-1"), ("content-type", AUTH_JSON)],
        Some(body.clone().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replay before recall: {json}");
    assert_eq!(json["message"]["body"], "secret-original");
    assert_eq!(json["message"]["mentions"], serde_json::json!([]));
    assert!(json["message"]["recalled_at"].is_null());
    assert!(json["message"].get("sequence").is_none());

    recall_human(&f.router, &f.token_a, &room_id, &msg_id).await;

    // History：tombstone 占原位置——body=null、mentions=[]、
    // recalled_at=首次时点；id/room/actor/reply_to/created_at 保留；无
    // sequence、无原正文。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view = &json["messages"][0];
    assert_eq!(view["id"], msg_id);
    assert_eq!(view["room_id"], room_id);
    assert!(
        view["body"].is_null(),
        "tombstone body must be null: {view}"
    );
    assert_eq!(view["mentions"], serde_json::json!([]));
    assert_eq!(view["actor"]["id"], f.human_a);
    assert!(view["reply_to_message_id"].is_null());
    assert!(
        view["recalled_at"].is_string(),
        "recalled_at must be RFC3339: {view}"
    );
    assert!(view["created_at"].is_string());
    assert!(view.get("sequence").is_none());
    let text = serde_json::to_string(&json).unwrap();
    assert!(
        !text.contains("secret-original"),
        "history must not leak the original body: {text}"
    );

    // 撤回后发送幂等重放：200 同 ID，视图为 tombstone、不泄原正文。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "m-1"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "replay after recall must be 200: {json}"
    );
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["message"]["id"], msg_id);
    assert!(
        json["message"]["body"].is_null(),
        "replay view must be tombstone: {json}"
    );

    // Backend Event：created 与 recalled 都返回 kind，message 均为
    // tombstone；无原正文。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events",
        Some(&format!("Bearer {backend_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "created + recalled");
    assert_eq!(events[0]["kind"], "message.created");
    assert_eq!(events[1]["kind"], "message.recalled");
    for event in events {
        assert!(
            event["message"]["body"].is_null(),
            "backend event must show tombstone: {event}"
        );
        assert_eq!(event["message"]["id"], msg_id);
    }
    let text = serde_json::to_string(&json).unwrap();
    assert!(
        !text.contains("secret-original"),
        "backend events must not leak the original body: {text}"
    );

    // Human Event：message.recalled 快照最小集；created 快照已持久变形
    // （body=null、recalled_at 首次时点、恒不含 mentions/sequence）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/human/events",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = json["events"].as_array().unwrap();
    let recalled = events
        .iter()
        .find(|e| e["kind"] == "message.recalled")
        .expect("recalled human event exists");
    let snap = &recalled["snapshot"];
    assert_eq!(snap["message_id"], msg_id);
    assert_eq!(snap["room_id"], room_id);
    assert!(snap["recalled_at"].is_number());
    assert!(
        snap.get("body").is_none(),
        "recalled snapshot is minimal: {snap}"
    );
    assert!(snap.get("mentions").is_none());
    assert!(snap.get("sequence").is_none());
    assert!(snap.get("source_id").is_none());
    let created = events
        .iter()
        .find(|e| e["kind"] == "message.created")
        .expect("created human event exists");
    let created_snap = &created["snapshot"];
    assert!(
        created_snap["body"].is_null(),
        "created snapshot must be tombstoned: {created_snap}"
    );
    assert!(created_snap["recalled_at"].is_number());
    assert!(created_snap.get("mentions").is_none());
    assert!(created_snap.get("sequence").is_none());
    let text = serde_json::to_string(&json).unwrap();
    assert!(
        !text.contains("secret-original"),
        "human events must not leak the original body: {text}"
    );
}

// ─────────────────────────── Completion / reply 相邻契约 ──────────────────

#[tokio::test]
async fn reply_to_tombstone_is_400_invalid_reply_target() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    let msg_id = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;

    recall_human(&f.router, &f.token_a, &room_id, &msg_id).await;

    // 撤回后新建回复该 target：统一 400 invalid_reply_target。
    let body = serde_json::json!({
        "body": "late reply",
        "reply_to_message_id": msg_id,
    })
    .to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "m-2"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "reply to recalled target: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_reply_target");
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(messages, 1, "rejected reply must leave no message");
}
