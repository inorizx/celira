//! Agent 读游标端点 HTTP 契约测试（CORE_V1_AGENT_READ_CURSOR.md，切片 M1）：
//! BackendAuth 认证（401 语义对齐既有 Backend 端点、Human token 一律拒绝）、
//! active membership 准入（非成员房间 404 room_not_found）、目标 Message 的
//! 可见性裁决（未知 / 他 Room / 不可见统一 400 invalid_read_cursor）、
//! 单调推进与 no-op、body 形状（未知字段 400、超限 413）、以及通讯录
//! `read_state` 与该端点的一致性。断言全走公共 HTTP 协议，不直读 SQLite。
//!
//! "Agent 的推进不产生任何 Human Event" 这条不在这里判：它的权威证据是
//! 存储层用例 `read_cursor_tests::agent_cursor_advances_without_any_human_event`
//! （fact / delivery 两层事实逐条数），本文件只判端点契约。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
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
    agent_token: String,
    /// Human + Agent A 的 Direct Room（A 是 active member）。
    direct_room: String,
    /// 只有 Human 的 Group Room（A 不是成员）。
    outsider_room: String,
    /// direct_room 里的两条 Human 消息（按 sequence 升序）。
    messages: Vec<String>,
}

/// 通过 schema 直接创建第二个 Human 并补发 Session（既有测试同款 fixture
/// 手法；断言面仍全走公共协议）。
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
    let (_human_b, _token_b) = add_human(&pool, &identity, "Bob").await;
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
    let human_token = a.token.as_str().to_owned();

    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/agents",
        &human_token,
        Some("rc-a-1"),
        Some(&serde_json::json!({"display_name": "Agent A"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create agent A: {json}");
    let agent_a = json["agent"]["id"].as_str().unwrap().to_owned();
    let agent_token = credential_for(&router, &human_token, &agent_a, "rc-b").await;

    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/direct-rooms",
        &human_token,
        Some("rc-d-1"),
        Some(&serde_json::json!({"agent_id": agent_a.clone()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create direct room: {json}");
    let direct_room = json["room"]["id"].as_str().unwrap().to_owned();

    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/groups",
        &human_token,
        Some("rc-g-1"),
        Some(&serde_json::json!({"name": "只有人的群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create group room: {json}");
    let outsider_room = json["room"]["id"].as_str().unwrap().to_owned();

    // direct_room 里两条 Human 消息（A 尚未读过 → 两条未读）。
    let mut messages = Vec::new();
    for (key, body) in [("rc-m-1", "你好"), ("rc-m-2", "在吗")] {
        let (status, json, _) = request_json(
            &router,
            Method::POST,
            &format!("/v1/rooms/{direct_room}/messages"),
            &human_token,
            Some(key),
            Some(&serde_json::json!({"body": body})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "send {body}: {json}");
        messages.push(json["message"]["id"].as_str().unwrap().to_owned());
    }

    Fixture {
        _dir: dir,
        router,
        human_token,
        agent_token,
        direct_room,
        outsider_room,
        messages,
    }
}

/// 创建 Binding + 签发 Credential，返回 backend token（公开 API 全链）。
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
        Some(&serde_json::json!({"label": "read cursor e2e"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create binding {agent_id}: {json}"
    );
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
    assert_eq!(
        status,
        StatusCode::CREATED,
        "issue credential {agent_id}: {json}"
    );
    json["token"].as_str().unwrap().to_owned()
}

async fn send(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method.clone()).uri(path);
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
    let _ = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json)
}

/// 走 Backend 读游标端点（Bearer = backend token；`body` 原样送字节）。
async fn put_cursor_raw(
    router: &axum::Router,
    token: Option<&str>,
    room: &str,
    body: Option<&serde_json::Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    headers.extend_from_slice(extra);
    send(
        router,
        Method::PUT,
        &format!("/v1/backend/rooms/{room}/read-cursor"),
        token.map(|t| format!("Bearer {t}")).as_deref(),
        &headers,
        body.map(|v| v.to_string().into_bytes()),
    )
    .await
}

async fn put_cursor(
    router: &axum::Router,
    token: &str,
    room: &str,
    message_id: &str,
) -> (StatusCode, serde_json::Value) {
    put_cursor_raw(
        router,
        Some(token),
        room,
        Some(&serde_json::json!({ "message_id": message_id })),
        &[],
    )
    .await
}

/// 走 Backend 通讯录端点，返回该房的 read_state 对象。
async fn directory_read_state(router: &axum::Router, token: &str, room: &str) -> serde_json::Value {
    let (status, json) = send(
        router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "directory: {json}");
    json["rooms"]
        .as_array()
        .and_then(|list| list.iter().find(|r| r["room_id"].as_str() == Some(room)))
        .unwrap_or_else(|| panic!("room {room} must be in the directory: {json}"))["read_state"]
        .clone()
}

async fn request_json(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: &str,
    key: Option<&str>,
    json_body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(key) = key {
        headers.push((KEY_HEADER, key));
    }
    if json_body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    let mut builder = Request::builder().method(method).uri(path);
    builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    for (name, value) in &headers {
        builder = builder.header(*name, *value);
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

/// 起点：A 在 direct_room 里两条未读、位置为空（通讯录投影）。
#[tokio::test]
async fn agent_starts_with_fresh_boundary_and_advances_monotonically() {
    let f = fixture().await;
    let state = directory_read_state(&f.router, &f.agent_token, &f.direct_room).await;
    assert_eq!(state["last_read_message_id"], serde_json::Value::Null);
    assert_eq!(state["unread_count"].as_i64(), Some(2));

    // 推进到第一条：advanced=true、位置 = 该条、未读 1；通讯录读到同一个值。
    let (status, json) =
        put_cursor(&f.router, &f.agent_token, &f.direct_room, &f.messages[0]).await;
    assert_eq!(status, StatusCode::OK, "advance: {json}");
    assert_eq!(json["advanced"].as_bool(), Some(true));
    assert_eq!(
        json["read_state"]["last_read_message_id"].as_str(),
        Some(f.messages[0].as_str())
    );
    assert_eq!(json["read_state"]["unread_count"].as_i64(), Some(1));
    let state = directory_read_state(&f.router, &f.agent_token, &f.direct_room).await;
    assert_eq!(
        state["last_read_message_id"].as_str(),
        Some(f.messages[0].as_str())
    );
    assert_eq!(state["unread_count"].as_i64(), Some(1));

    // 推进到第二条：未读 0。
    let (status, json) =
        put_cursor(&f.router, &f.agent_token, &f.direct_room, &f.messages[1]).await;
    assert_eq!(status, StatusCode::OK, "advance: {json}");
    assert_eq!(json["advanced"].as_bool(), Some(true));
    assert_eq!(json["read_state"]["unread_count"].as_i64(), Some(0));

    // 同值 / 旧值：成功 no-op（200 advanced=false），位置不倒退。
    for target in [&f.messages[1], &f.messages[0]] {
        let (status, json) = put_cursor(&f.router, &f.agent_token, &f.direct_room, target).await;
        assert_eq!(status, StatusCode::OK, "no-op: {json}");
        assert_eq!(json["advanced"].as_bool(), Some(false));
        assert_eq!(
            json["read_state"]["last_read_message_id"].as_str(),
            Some(f.messages[1].as_str()),
            "no-op must not move the cursor backwards"
        );
    }
}

/// Agent 自己发的消息不算自己的未读（与 Human 同一投影口径）。
#[tokio::test]
async fn agent_own_messages_do_not_count_as_unread() {
    let f = fixture().await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{}/messages", f.direct_room),
        &f.agent_token,
        Some("rc-say-1"),
        Some(&serde_json::json!({"body": "我在"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent say: {json}");
    let state = directory_read_state(&f.router, &f.agent_token, &f.direct_room).await;
    assert_eq!(
        state["unread_count"].as_i64(),
        Some(2),
        "agent 自己发的那条不计入自己的未读"
    );
}

/// 认证面：缺 header 401 authentication_required；Human token 与未知 token
/// 一律 401 invalid_credential（同一文案，不形成存在性 oracle）。
#[tokio::test]
async fn auth_is_backend_only_and_human_tokens_are_rejected() {
    let f = fixture().await;
    let (status, json) = put_cursor_raw(
        &f.router,
        None,
        &f.direct_room,
        Some(&serde_json::json!({"message_id": f.messages[0]})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no auth: {json}");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("authentication_required")
    );

    for token in [f.human_token.as_str(), "clr_bc_not-a-real-token"] {
        let (status, json) = put_cursor_raw(
            &f.router,
            Some(token),
            &f.direct_room,
            Some(&serde_json::json!({"message_id": f.messages[0]})),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "token {token}: {json}");
        assert_eq!(json["error"]["code"].as_str(), Some("invalid_credential"));
    }
}

/// 准入与目标裁决：非成员房间 / 未知房间 404 room_not_found；坏 id 400
/// invalid_request；未知或他 Room 的消息 400 invalid_read_cursor。
#[tokio::test]
async fn non_member_and_invalid_targets_fail_closed() {
    let f = fixture().await;

    // A 不是这个群的成员 → 404（与 Human 侧同一判据，不泄露房间是否存在）。
    let (status, json) =
        put_cursor(&f.router, &f.agent_token, &f.outsider_room, &f.messages[0]).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "non-member: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("room_not_found"));

    // 未知房间（规范 UUID）→ 同一个 404。
    let (status, json) = put_cursor(
        &f.router,
        &f.agent_token,
        &Uuid::new_v4().to_string(),
        &f.messages[0],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown room: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("room_not_found"));

    // 坏 room id / 坏 message id → 400 invalid_request。
    let (status, json) = put_cursor(&f.router, &f.agent_token, "not-a-uuid", &f.messages[0]).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "bad room id: {json}");
    let (status, json) = put_cursor(&f.router, &f.agent_token, &f.direct_room, "not-a-uuid").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad message id: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_request"));

    // 未知 Message（规范 UUID）与他 Room 的真实 Message → 400 invalid_read_cursor。
    let (status, json) = put_cursor(
        &f.router,
        &f.agent_token,
        &f.direct_room,
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown message: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_read_cursor"));

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/messages", f.outsider_room),
        &f.human_token,
        Some("rc-out-1"),
        Some(&serde_json::json!({"body": "别人房间的消息"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "outsider message: {json}");
    let outsider_message = json["message"]["id"].as_str().unwrap().to_owned();
    let (status, json) =
        put_cursor(&f.router, &f.agent_token, &f.direct_room, &outsider_message).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "other-room message: {json}"
    );
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_read_cursor"));
}

/// body 形状：未知字段 400 invalid_request；超过 1 KiB 413 payload_too_large；
/// 非 JSON 400。失败不改变 cursor。
#[tokio::test]
async fn body_contract_is_enforced() {
    let f = fixture().await;

    let (status, json) = put_cursor_raw(
        &f.router,
        Some(&f.agent_token),
        &f.direct_room,
        Some(&serde_json::json!({"message_id": f.messages[0], "sequence": 1})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown field: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_request"));

    let (status, json) = put_cursor_raw(
        &f.router,
        Some(&f.agent_token),
        &f.direct_room,
        None,
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty body: {json}");

    let big = serde_json::json!({"message_id": f.messages[0], "pad": "x".repeat(2048)});
    let (status, json) = put_cursor_raw(
        &f.router,
        Some(&f.agent_token),
        &f.direct_room,
        Some(&big),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "oversized body: {json}"
    );
    assert_eq!(json["error"]["code"].as_str(), Some("payload_too_large"));

    // 三次失败之后位置仍在起点（失败不改 cursor）。
    let state = directory_read_state(&f.router, &f.agent_token, &f.direct_room).await;
    assert_eq!(state["last_read_message_id"], serde_json::Value::Null);
    assert_eq!(state["unread_count"].as_i64(), Some(2));
}

/// Human 侧读游标端点语义不变（回归）：同一房间、同一目标，Human 仍能推进，
/// 且 Human 的推进与 Agent 的推进互不影响（各自的位置各自记）。
#[tokio::test]
async fn human_read_cursor_endpoint_is_unchanged() {
    let f = fixture().await;

    // Agent 先推进到第二条。
    let (status, _) = put_cursor(&f.router, &f.agent_token, &f.direct_room, &f.messages[1]).await;
    assert_eq!(status, StatusCode::OK);

    // Human 走既有端点推进到第一条：200 advanced=true（语义一字不变）。
    let (status, json, _) = request_json(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{}/read-cursor", f.direct_room),
        &f.human_token,
        None,
        Some(&serde_json::json!({"message_id": f.messages[0]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "human advance: {json}");
    assert_eq!(json["advanced"].as_bool(), Some(true));
    assert_eq!(
        json["read_state"]["last_read_message_id"].as_str(),
        Some(f.messages[0].as_str()),
        "Human 响应形状与语义一字不变（read_state.last_read_message_id）"
    );

    // 两边各自的位置：Agent 在第二条，Human 在第一条（互不覆盖）。
    let agent_state = directory_read_state(&f.router, &f.agent_token, &f.direct_room).await;
    assert_eq!(
        agent_state["last_read_message_id"].as_str(),
        Some(f.messages[1].as_str())
    );
    let (_, rooms, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/rooms",
        &f.human_token,
        None,
        None,
    )
    .await;
    let human_room = rooms["rooms"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|r| r["room"]["id"].as_str() == Some(f.direct_room.as_str()))
        })
        .expect("direct room in human list");
    assert_eq!(
        human_room["read_state"]["last_read_message_id"].as_str(),
        Some(f.messages[0].as_str())
    );
}
