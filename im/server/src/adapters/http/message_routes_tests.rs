//! Message HTTP 契约测试（CORE_V1_MESSAGES.md 8.x/9.x）：首次 201、幂等
//! 重放 200/冲突 409、401/404/400/413 错误矩阵、正文字节边界、路由 128 KiB
//! 上限与既有 4 KiB 上限并存、History keyset 分页、actor 公共投影与
//! idempotency key 从不泄露。

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
const AUTH_JSON: &str = "application/json";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    token_a: String,
    human_a: String,
    token_b: String,
    pool: SqlitePool,
    identity: SqliteIdentityStore,
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
    Fixture {
        _dir: dir,
        router,
        token_a: a.token.as_str().to_owned(),
        human_a: a.participant.id.as_str().to_owned(),
        token_b,
        pool,
        identity,
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

/// 通过公开 API 创建 Agent（Alice 或 Bob），返回 Agent ID。
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

async fn send_message(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    body_text: &str,
    key: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let body = serde_json::json!({ "body": body_text }).to_string();
    send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await
}

async fn get_history(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    query: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/messages{query}"),
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await
}

fn assert_error_envelope(
    status: StatusCode,
    json: &serde_json::Value,
    request_id: &Option<String>,
) {
    assert!(!status.is_success());
    assert_eq!(
        json["error"]["request_id"],
        request_id.as_deref().unwrap_or_default(),
        "error.request_id must equal x-request-id"
    );
}

/// 消息视图必须只含公开字段：不得出现 idempotency key、内部协调列或任何
/// sequence（反向测试：公共协议不暴露 sequence）。
fn assert_public_message(json: &serde_json::Value) {
    for view in ["message", "messages"] {
        if let Some(item) = json.get(view) {
            if item.is_array() {
                for entry in item.as_array().unwrap() {
                    assert!(entry.get("idempotency").is_none());
                    assert!(entry.get("idempotency_key").is_none());
                    assert!(entry.get("sequence").is_none());
                }
            } else {
                assert!(item.get("idempotency").is_none());
                assert!(item.get("idempotency_key").is_none());
                assert!(item.get("sequence").is_none());
            }
        }
    }
}

#[tokio::test]
async fn first_send_returns_201_with_actor_from_session() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, request_id) =
        send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(
        json["request_id"],
        request_id.as_deref().unwrap_or_default()
    );
    assert_eq!(json["message"]["room_id"], room_id);
    assert_eq!(json["message"]["body"], "hello");
    assert_eq!(json["message"]["actor"]["id"], f.human_a);
    assert_eq!(json["message"]["actor"]["kind"], "human");
    assert_eq!(json["message"]["actor"]["display_name"], "Alice");
    assert_eq!(json["message"]["actor"]["status"], "active");
    assert!(
        json["message"]["created_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    assert_eq!(json["message"]["id"].as_str().unwrap().len(), 36);
    assert_public_message(&json);
}

#[tokio::test]
async fn replay_same_key_same_request_returns_200_same_id_without_duplicate() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (first_status, first_json, _) =
        send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    assert_eq!(first_status, StatusCode::CREATED);

    let (status, json, _) = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["message"]["id"], first_json["message"]["id"]);
    assert_eq!(
        json["message"]["created_at"],
        first_json["message"]["created_at"]
    );
    assert_eq!(json["message"]["body"], first_json["message"]["body"]);

    let (hstatus, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(hstatus, StatusCode::OK);
    assert_eq!(history["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn same_key_different_body_conflicts_and_first_fact_unchanged() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, _) = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let first_id = json["message"]["id"].clone();

    let (status, json, request_id) =
        send_message(&f.router, &f.token_a, &room_id, "changed", "m-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    assert_error_envelope(status, &json, &request_id);

    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 1);
    assert_eq!(history["messages"][0]["id"], first_id);
    assert_eq!(history["messages"][0]["body"], "hello");
}

#[tokio::test]
async fn same_key_different_room_conflicts_without_new_message() {
    let f = fixture().await;
    let agent_1 = create_agent(&f.router, &f.token_a, "甲", "a-k1").await;
    let agent_2 = create_agent(&f.router, &f.token_a, "乙", "a-k2").await;
    let room_1 = create_room(&f.router, &f.token_a, &agent_1, "r-k1").await;
    let room_2 = create_room(&f.router, &f.token_a, &agent_2, "r-k2").await;

    send_message(&f.router, &f.token_a, &room_1, "hello", "m-1").await;
    let (status, json, _) = send_message(&f.router, &f.token_a, &room_2, "hello", "m-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    let (_, history_2, _) = get_history(&f.router, &f.token_a, &room_2, "").await;
    assert_eq!(history_2["messages"].as_array().unwrap().len(), 0);
    let (_, history_1, _) = get_history(&f.router, &f.token_a, &room_1, "").await;
    assert_eq!(history_1["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn same_key_different_actor_is_independent() {
    let f = fixture().await;
    let alice_agent = create_agent(&f.router, &f.token_a, "甲的助手", "a-k").await;
    let bob_agent = create_agent(&f.router, &f.token_b, "乙的助手", "b-k").await;
    let alice_room = create_room(&f.router, &f.token_a, &alice_agent, "r-k-a").await;
    let bob_room = create_room(&f.router, &f.token_b, &bob_agent, "r-k-b").await;

    send_message(&f.router, &f.token_a, &alice_room, "alice says", "shared").await;
    let (status, json, _) =
        send_message(&f.router, &f.token_b, &bob_room, "bob says", "shared").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "same key under another actor is a fresh fact"
    );
    assert_eq!(json["message"]["body"], "bob says");
    assert!(json["message"]["id"].as_str().is_some());
}

#[tokio::test]
async fn non_member_send_is_404_without_side_effect() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;
    send_message(&f.router, &f.token_a, &room_id, "private", "m-1").await;
    let (status, json, request_id) =
        send_message(&f.router, &f.token_b, &room_id, "intrude", "bob-k").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");
    assert_error_envelope(status, &json, &request_id);

    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 1);
    let (_, history_b, _) = get_history(&f.router, &f.token_b, &room_id, "").await;
    assert_eq!(history_b["error"]["code"], "room_not_found");
}

#[tokio::test]
async fn unknown_and_bad_room_ids_are_404() {
    let f = fixture().await;
    let unknown = Uuid::new_v4().to_string();
    for room in [
        &unknown,
        "not-a-uuid",
        "01234567-89ab-cdef-0123-456789abcdef",
    ] {
        let (status, json, request_id) =
            send_message(&f.router, &f.token_a, room, "hello", "m-x").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "room {room}");
        assert_eq!(json["error"]["code"], "room_not_found");
        assert_error_envelope(status, &json, &request_id);
    }
}

#[tokio::test]
async fn unauthenticated_and_invalid_session_send_are_401() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        None,
        &[(KEY_HEADER, "m-1"), ("content-type", AUTH_JSON)],
        Some(br#"{"body":"hello"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some("Bearer invalid-token"),
        &[(KEY_HEADER, "m-1"), ("content-type", AUTH_JSON)],
        Some(br#"{"body":"hello"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");
}

#[tokio::test]
async fn body_validation_returns_400_invalid_message_body_without_facts() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    for (i, body_text) in ["", "a\u{0}b", &"a".repeat(16385)].iter().enumerate() {
        let (status, json, request_id) = send_message(
            &f.router,
            &f.token_a,
            &room_id,
            body_text,
            &format!("bad-{i}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {i:?}");
        assert_eq!(json["error"]["code"], "invalid_message_body");
        assert_error_envelope(status, &json, &request_id);
    }

    // 纯空白正文合法（不做 trim，原样保存）。
    let (status, json, _) = send_message(&f.router, &f.token_a, &room_id, " \t\n\r ", "ws-1").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["message"]["body"], " \t\n\r ");

    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "failed attempts must leave no facts");
    assert_eq!(messages[0]["body"], " \t\n\r ");
}

#[tokio::test]
async fn utf8_byte_boundary_accepts_16384_and_rejects_16385() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, _) =
        send_message(&f.router, &f.token_a, &room_id, &"a".repeat(16384), "ok-1").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "16384 ASCII bytes must be accepted"
    );
    assert_eq!(json["message"]["body"], "a".repeat(16384));

    let (status, json, _) =
        send_message(&f.router, &f.token_a, &room_id, &"a".repeat(16385), "bad-1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_message_body");

    // 多字节 UTF-8：16383 字节（5461 个中文字符）合法。
    let (status, json, _) =
        send_message(&f.router, &f.token_a, &room_id, &"中".repeat(5461), "ok-2").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "16383 utf8 bytes must be accepted"
    );
    assert_eq!(json["message"]["body"], "中".repeat(5461));

    // 16386 字节（5462 个中文字符）必须按字节拒绝，不能按字符数放行。
    let (status, json, _) =
        send_message(&f.router, &f.token_a, &room_id, &"中".repeat(5462), "bad-2").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_message_body");

    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unicode_and_whitespace_round_trip_verbatim() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let bodies = [
        "line1\nline2\r\nline3\ttab",
        "😀 emoji 你好，世界",
        "trailing space ",
        "leading space",
        "混合 Mixed 123",
    ];
    for (i, body) in bodies.iter().enumerate() {
        let (status, json, _) =
            send_message(&f.router, &f.token_a, &room_id, body, &format!("u-{i}")).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["message"]["body"], *body, "must be verbatim");
    }
    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    let round_trip: Vec<String> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(round_trip, bodies);
}

#[tokio::test]
async fn message_route_accepts_beyond_4kib_while_agent_route_stays_4kib() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    // Message 路由：正文 9000 字节（JSON 整体超过 4 KiB，远小于 128 KiB）→ 201。
    let (status, json, _) =
        send_message(&f.router, &f.token_a, &room_id, &"x".repeat(9000), "big-1").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "message route must accept >4KiB"
    );
    assert_eq!(json["message"]["body"], "x".repeat(9000));

    // Message 路由：整体 JSON 超过 128 KiB → 413 payload_too_large。
    let oversized = "x".repeat(131_100);
    let (status, json, request_id) =
        send_message(&f.router, &f.token_a, &room_id, &oversized, "huge-1").await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json["error"]["code"], "payload_too_large");
    assert_error_envelope(status, &json, &request_id);

    // Agent 创建路由：4 KiB 上限保持不变（用 JSON 空白填充超过上限，
    // 413 在正文缓冲层先于 JSON 解析触发）。
    let body = format!("{} {}", " ".repeat(5000), r#"{"display_name":"Bob"}"#);
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "a-big"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "agent route must keep 4KiB"
    );
    assert_eq!(json["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn history_first_page_latest_ascending_with_keyset_cursor_pagination() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;
    let mut bodies = Vec::new();
    for i in 1..=7 {
        let body = format!("msg {i}");
        send_message(&f.router, &f.token_a, &room_id, &body, &format!("m-{i}")).await;
        bodies.push(body);
    }

    // 首屏默认 limit=50：全部 7 条，升序，无 next_before。
    let (status, json, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(status, StatusCode::OK);
    let page_bodies: Vec<String> = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page_bodies, bodies);
    assert_eq!(json["next_before"], serde_json::Value::Null);

    // 首屏 limit=3：最新 3 条升序；next_before 是当前页第一条（最旧一条）
    // 的 opaque cursor 字符串。
    let (_, page1, _) = get_history(&f.router, &f.token_a, &room_id, "?limit=3").await;
    let page1_bodies: Vec<String> = page1["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page1_bodies, vec!["msg 5", "msg 6", "msg 7"]);
    let next1 = page1["next_before"].as_str().expect("cursor string");
    assert!(next1.starts_with("c1.m."), "opaque cursor: {next1}");

    // 翻页：把 next_before 原样回传，无重复、无跳过地读完全部。
    let (_, page2, _) = get_history(
        &f.router,
        &f.token_a,
        &room_id,
        &format!("?limit=3&before={next1}"),
    )
    .await;
    let page2_bodies: Vec<String> = page2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page2_bodies, vec!["msg 2", "msg 3", "msg 4"]);
    let next2 = page2["next_before"].as_str().expect("cursor string");

    let (_, page3, _) = get_history(
        &f.router,
        &f.token_a,
        &room_id,
        &format!("?limit=3&before={next2}"),
    )
    .await;
    let page3_bodies: Vec<String> = page3["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page3_bodies, vec!["msg 1"]);
    assert_eq!(page3["next_before"], serde_json::Value::Null);

    let all: Vec<String> = [&page1, &page2, &page3]
        .iter()
        .flat_map(|p| {
            p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["body"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(
        all,
        vec![
            "msg 5", "msg 6", "msg 7", "msg 2", "msg 3", "msg 4", "msg 1"
        ]
    );
    assert_public_message(&page1);
}

#[tokio::test]
async fn empty_room_history_is_empty_page() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["messages"], serde_json::json!([]));
    assert_eq!(json["next_before"], serde_json::Value::Null);
}

#[tokio::test]
async fn invalid_paging_parameters_are_400_invalid_request() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    for query in [
        "?limit=0",
        "?limit=101",
        "?limit=abc",
        "?limit=1&limit=2",
        "?before=0",
        "?before=-1",
        "?before=abc",
        "?before=c1.m.start",
        "?before=c1.he.550e8400-e29b-41d4-a716-446655440001",
        "?before=c1.be.550e8400-e29b-41d4-a716-446655440001",
        "?before=c1.m.550E8400-E29B-41D4-A716-446655440000",
        "?before=c2.m.550e8400-e29b-41d4-a716-446655440000",
        "?before=c1.m.550e8400-e29b-41d4-a716-446655440000",
        "?before=1&before=2",
    ] {
        let (status, json, request_id) = get_history(&f.router, &f.token_a, &room_id, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "query {query}");
        assert_eq!(json["error"]["code"], "invalid_request");
        assert_error_envelope(status, &json, &request_id);
    }
}

#[tokio::test]
async fn history_cursor_scope_errors_are_400_invalid_request_without_leaking_existence() {
    let f = fixture().await;
    let agent_1 = create_agent(&f.router, &f.token_a, "甲", "a-k1").await;
    let agent_2 = create_agent(&f.router, &f.token_a, "乙", "a-k2").await;
    let room_1 = create_room(&f.router, &f.token_a, &agent_1, "r-k1").await;
    let room_2 = create_room(&f.router, &f.token_a, &agent_2, "r-k2").await;
    send_message(&f.router, &f.token_a, &room_1, "in room 1", "m-1").await;
    let (_, history, _) = get_history(&f.router, &f.token_a, &room_1, "").await;
    let msg_id = history["messages"][0]["id"].as_str().unwrap().to_owned();
    let cursor = format!("c1.m.{msg_id}");

    // 同 Room 可见 cursor：可用且能正确翻页。
    let (status, _json, _) =
        get_history(&f.router, &f.token_a, &room_1, &format!("?before={cursor}")).await;
    assert_eq!(status, StatusCode::OK);

    // 跨 Room cursor：同一 Message 在另一个 Room 的 History 上不可解析。
    let (status, json, request_id) =
        get_history(&f.router, &f.token_a, &room_2, &format!("?before={cursor}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_error_envelope(status, &json, &request_id);

    // 未知 Message cursor 与非成员使用同 Room cursor：统一 400，不泄露
    // 存在性（两种情形响应完全一致）。
    let unknown = format!("c1.m.{}", Uuid::new_v4());
    for query in [format!("?before={unknown}"), format!("?before={cursor}")] {
        let (status, json, _) = get_history(&f.router, &f.token_a, &room_2, &query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "query {query}");
        assert_eq!(json["error"]["code"], "invalid_request");
    }
    let (status, json, _) =
        get_history(&f.router, &f.token_b, &room_1, &format!("?before={cursor}")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "non-member cursor must be 400"
    );
    assert_eq!(json["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn history_authorization_is_404_and_401_respectively() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;
    send_message(&f.router, &f.token_a, &room_id, "private", "m-1").await;
    // 非成员读取：404，不泄露房间或消息是否存在。
    let (status, json, _) = get_history(&f.router, &f.token_b, &room_id, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");

    // unknown room：404。
    let (status, json, _) =
        get_history(&f.router, &f.token_a, &Uuid::new_v4().to_string(), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");

    // 未认证读取：401。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/messages"),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");
}

#[tokio::test]
async fn agent_revoke_keeps_history_and_allows_new_human_messages() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;
    let (_, first, _) = send_message(&f.router, &f.token_a, &room_id, "before revoke", "m-1").await;

    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Human 仍可读取既有历史，Message actor ID 与正文不变。
    let (status, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(status, StatusCode::OK);
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], first["message"]["id"]);
    assert_eq!(messages[0]["body"], "before revoke");

    // 新 Human Message 只依据 actor/Room/Membership 当前状态裁决。
    let (status, after, _) =
        send_message(&f.router, &f.token_a, &room_id, "after revoke", "m-2").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(after["message"]["body"], "after revoke");
    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn malformed_json_and_unknown_fields_are_400_invalid_request() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;
    let path = format!("/v1/rooms/{room_id}/messages");
    let auth = format!("Bearer {}", f.token_a);

    for raw in [
        "not json",
        r#"{"body": 42}"#,
        r#"{"body": "hello", "extra": 1}"#,
        r#"{}"#,
        r#"[]"#,
    ] {
        let (status, json, request_id) = send(
            &f.router,
            Method::POST,
            &path,
            Some(&auth),
            &[(KEY_HEADER, "m-1"), ("content-type", AUTH_JSON)],
            Some(raw.as_bytes().to_vec()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "payload {raw}");
        assert_eq!(json["error"]["code"], "invalid_request");
        assert_error_envelope(status, &json, &request_id);
    }
}

#[tokio::test]
async fn idempotency_key_header_validation() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;
    let path = format!("/v1/rooms/{room_id}/messages");
    let auth = format!("Bearer {}", f.token_a);

    // 缺失 key：400 idempotency_key_required。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &path,
        Some(&auth),
        &[("content-type", AUTH_JSON)],
        Some(br#"{"body":"hello"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "idempotency_key_required");

    // 格式非法 key（超长 / 逗号 / 空白）：400 invalid_idempotency_key。
    for bad_key in [&"a".repeat(129), "a,b", " leading"] {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            &path,
            Some(&auth),
            &[(KEY_HEADER, bad_key), ("content-type", AUTH_JSON)],
            Some(br#"{"body":"hello"}"#.to_vec()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "key {bad_key:?}");
        assert_eq!(json["error"]["code"], "invalid_idempotency_key");
    }
}

// ===== mentions：HTTP 契约 =====

#[tokio::test]
async fn mentions_in_group_messages_via_http() {
    let f = fixture().await;
    // 建立 accepted 联系人（group 邀请 Human 用）与一个 owned Agent。
    let (bob_id, bob_token) = add_human(&f.pool, &f.identity, "Bob").await;
    let (status, _json, _) = send(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "c-k")],
        Some(
            serde_json::json!({ "participant_id": bob_id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", f.human_a),
        Some(&format!("Bearer {bob_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "accept: {json}");
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "m-agent").await;

    // 建群并邀请 bob + agent。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "g-k")],
        Some(serde_json::json!({ "name": "群" }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = json["room"]["id"].as_str().unwrap().to_owned();
    for participant in [bob_id.clone(), agent_id.clone()] {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            &format!("/v1/rooms/{group_id}/members"),
            Some(&format!("Bearer {}", f.token_a)),
            &[
                ("content-type", "application/json"),
                (KEY_HEADER, &format!("inv-{participant}")),
            ],
            Some(
                serde_json::json!({ "participant_id": participant })
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "invite: {json}");
    }

    // 带 mention 发送 → 201；响应与 History 返回规范 mentions。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-1")],
        Some(
            serde_json::json!({ "body": "@助手 你好", "mentions": [agent_id] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mention message: {json}");
    assert_eq!(json["message"]["mentions"], serde_json::json!([agent_id]));
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["messages"][0]["mentions"],
        serde_json::json!([agent_id])
    );

    // 缺省 mentions → 空数组。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-2")],
        Some(
            serde_json::json!({ "body": "普通" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["message"]["mentions"], serde_json::json!([]));

    // 重复 mention 目标 → 400 invalid_mention；bad uuid → 400。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-3")],
        Some(
            serde_json::json!({ "body": "x", "mentions": [agent_id, agent_id] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "duplicate mention: {json}");
    assert_eq!(json["error"]["code"], "invalid_mention");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-4")],
        Some(
            serde_json::json!({ "body": "x", "mentions": ["not-a-uuid"] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad mention id: {json}");
    assert_eq!(json["error"]["code"], "invalid_mention");

    // 只改变 mention 集合的幂等冲突 → 409 idempotency_conflict。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-1")],
        Some(
            serde_json::json!({ "body": "@助手 你好", "mentions": [] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "mention-set change: {json}");
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    // 同 key 同 mention 集合重放 → 200。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-1")],
        Some(
            serde_json::json!({ "body": "@助手 你好", "mentions": [agent_id] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "mention replay: {json}");
    assert_eq!(json["idempotency_replayed"], true);

    // human_direct 携带 mentions → 400 invalid_mention。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k")],
        Some(
            serde_json::json!({ "human_id": bob_id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let hd_id = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{hd_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-m-1")],
        Some(
            serde_json::json!({ "body": "x", "mentions": [agent_id] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "mentions in human direct: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_mention");

    // 非法 mention 后 sequence 未占用：下一条合法消息 sequence 连续。
    let (status, _json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-5")],
        Some(
            serde_json::json!({ "body": "@助手 继续", "mentions": [agent_id] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // 非法 mention 后无副作用：历史只有恰好 3 条消息（全部来自成功请求），
    // 数据库探针证明 Room sequence 连续无缺口（内部事实，不进入公共协议）。
    let (_, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    let bodies: Vec<String> = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(bodies.len(), 3, "no message from failed mentions");
    let max_seq: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM messages WHERE room_id = ?")
            .bind(&group_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(max_seq, 3, "no sequence gaps after failed mentions");
}

/// 65 个 Mention 目标在进入持久化之前被 400 invalid_mention 拒绝，且不
/// 留下 Message / Mention / Event / sequence。数量上限守卫先于 UUID 解析
/// 与存储层裁决，因此 HTTP 层无需先构造 64 个真实 Agent 成员。
#[tokio::test]
async fn over_64_mentions_rejected_before_persistence_via_http() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "m-agent").await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "g-k")],
        Some(serde_json::json!({ "name": "群" }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-65-agent"),
        ],
        Some(
            serde_json::json!({ "participant_id": agent_id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite: {json}");

    let mentions_65: Vec<String> = (0..65).map(|_| Uuid::new_v4().to_string()).collect();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-65")],
        Some(
            serde_json::json!({ "body": "@全员 回答", "mentions": mentions_65 })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "65 mentions must be rejected: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_mention");

    // 无 Message / Mention / Event / sequence 副作用：下一条合法消息
    // sequence=1 且数据库事实为空。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-ok")],
        Some(
            serde_json::json!({ "body": "@助手 继续", "mentions": [agent_id] })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "valid message: {json}");
    assert_eq!(json["message"]["mentions"], serde_json::json!([agent_id]));
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        messages, 1,
        "no message from the rejected 65-mention request"
    );
    // 数据库探针：失败请求不消耗 Room sequence（内部事实，不进入公共协议）。
    let seq: i64 = sqlx::query_scalar("SELECT sequence FROM messages LIMIT 1")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(seq, 1, "no sequence consumed");
    let mentions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions, 1, "only the valid mention persisted");
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events, 1, "only the valid mention generated an event");
}

// ===== Reply / Quote 切片（CORE_V1_REPLIES_AND_QUOTES.md）=====

/// 带 reply_to_message_id 的发送。
async fn send_reply(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    body_text: &str,
    reply_to: &str,
    key: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let body = serde_json::json!({
        "body": body_text,
        "reply_to_message_id": reply_to,
    })
    .to_string();
    send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await
}

#[tokio::test]
async fn reply_round_trip_replay_and_conflict_via_http() {
    let f = fixture().await;
    let agent_1 = create_agent(&f.router, &f.token_a, "甲", "a-k1").await;
    let agent_2 = create_agent(&f.router, &f.token_a, "乙", "a-k2").await;
    let room_1 = create_room(&f.router, &f.token_a, &agent_1, "r-1").await;
    let room_2 = create_room(&f.router, &f.token_a, &agent_2, "r-2").await;

    // 普通消息：reply_to_message_id 字段恒存在且为 null。
    let (status, json, _) = send_message(&f.router, &f.token_a, &room_1, "目标", "m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        json["message"]["reply_to_message_id"],
        serde_json::Value::Null
    );
    let target_id = json["message"]["id"].as_str().unwrap().to_owned();

    // 合法回复 → 201，响应携带 target ID。
    let (status, json, _) =
        send_reply(&f.router, &f.token_a, &room_1, "回复", &target_id, "m-2").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        json["message"]["reply_to_message_id"],
        serde_json::json!(target_id)
    );
    let reply_id = json["message"]["id"].as_str().unwrap().to_owned();

    // History：同一 source 携带 target ID，且不嵌入 target 正文/序列。
    let (status, json, _) = get_history(&f.router, &f.token_a, &room_1, "").await;
    assert_eq!(status, StatusCode::OK);
    let reply_view = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == serde_json::json!(reply_id))
        .expect("reply in history");
    assert_eq!(
        reply_view["reply_to_message_id"],
        serde_json::json!(target_id)
    );
    assert!(
        reply_view.get("quoted_body").is_none() && reply_view.get("target_sequence").is_none(),
        "history must not embed target content"
    );

    // 同 key 同完整请求 → 200 重放，同 Message ID / target。
    let (status, json, _) =
        send_reply(&f.router, &f.token_a, &room_1, "回复", &target_id, "m-2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["message"]["id"], serde_json::json!(reply_id));
    assert_eq!(
        json["message"]["reply_to_message_id"],
        serde_json::json!(target_id)
    );

    // 同 key 改 reply target → 409，既有事实不变。
    let (status, json, _) =
        send_reply(&f.router, &f.token_a, &room_1, "回复", &room_2, "m-2").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(replies, 1);
}

#[tokio::test]
async fn reply_target_error_surface_via_http() {
    let f = fixture().await;
    let agent_1 = create_agent(&f.router, &f.token_a, "甲", "a-k1").await;
    let agent_2 = create_agent(&f.router, &f.token_a, "乙", "a-k2").await;
    let room_1 = create_room(&f.router, &f.token_a, &agent_1, "r-1").await;
    let room_2 = create_room(&f.router, &f.token_a, &agent_2, "r-2").await;
    let (status, json, _) = send_message(&f.router, &f.token_a, &room_1, "目标", "m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let target_id = json["message"]["id"].as_str().unwrap().to_owned();

    // bad UUID 格式 → 400 invalid_request。
    let (status, json, request_id) =
        send_reply(&f.router, &f.token_a, &room_1, "回", "not-a-uuid", "m-x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_error_envelope(status, &json, &request_id);

    // 未知 target（规范 UUID 但不存在）→ 400 invalid_reply_target。
    let (status, json, _) = send_reply(
        &f.router,
        &f.token_a,
        &room_1,
        "回",
        &Uuid::new_v4().to_string(),
        "m-y",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_reply_target");

    // 跨 Room target → 400 invalid_reply_target（不泄露 target 归属）。
    let (status, json, _) =
        send_reply(&f.router, &f.token_a, &room_2, "回", &target_id, "m-z").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_reply_target");

    // 全部失败无副作用：无 Message / reply target / sequence 被消耗。
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(messages, 1);
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(replies, 0);
    let (status, _json, _) = send_message(&f.router, &f.token_a, &room_2, "后续", "m-w").await;
    assert_eq!(status, StatusCode::CREATED);
    // 数据库探针：失败回复不消耗 Room sequence（内部事实，不进入公共协议）。
    let seq: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM messages WHERE room_id = ?")
            .bind(&room_2)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(seq, 1, "failed replies must not consume sequences");
}

#[tokio::test]
async fn reply_target_rejects_other_fields_and_keeps_unknown_field_surface() {
    let f = fixture().await;
    let agent_1 = create_agent(&f.router, &f.token_a, "甲", "a-k1").await;
    let room_1 = create_room(&f.router, &f.token_a, &agent_1, "r-1").await;
    let (status, json, _) = send_message(&f.router, &f.token_a, &room_1, "目标", "m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let target_id = json["message"]["id"].as_str().unwrap().to_owned();

    // 客户端不能提交 target Room / target actor / quoted body。
    for field in [
        "target_room_id",
        "quoted_body",
        "target_sequence",
        "reply_actor_id",
    ] {
        let body = serde_json::json!({
            "body": "回",
            "reply_to_message_id": target_id,
            field: "something",
        })
        .to_string();
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            &format!("/v1/rooms/{room_1}/messages"),
            Some(&format!("Bearer {}", f.token_a)),
            &[(KEY_HEADER, "m-x"), ("content-type", AUTH_JSON)],
            Some(body.into_bytes()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "unknown field {field}");
        assert_eq!(json["error"]["code"], "invalid_request");
    }
    // 显式 null 等价于普通消息。
    let body = serde_json::json!({
        "body": "普通",
        "mentions": [],
        "reply_to_message_id": null,
    })
    .to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_1}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "m-null"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        json["message"]["reply_to_message_id"],
        serde_json::Value::Null
    );
}

// ---------------------------------------------------------------------------
// Backend agent 主动发消息端点（CORE_V1_AGENT_SPEAK.md，切片 A 房内主动说话）
// ---------------------------------------------------------------------------

/// 通过公开 API 创建 Backend Credential（Binding + 首次签发），返回原始 token。
/// `tag` 参与幂等键，同一 Human 下多 Agent 并发创建互不冲突。
async fn create_backend_token(
    router: &axum::Router,
    human_token: &str,
    agent_id: &str,
    tag: &str,
) -> String {
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(&format!("Bearer {human_token}")),
        &[
            (KEY_HEADER, &format!("agent-say-b-{tag}").to_string()),
            ("content-type", AUTH_JSON),
        ],
        Some(
            serde_json::json!({ "label": "agent-say" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        Some(&format!("Bearer {human_token}")),
        &[
            (KEY_HEADER, &format!("agent-say-c-{tag}").to_string()),
            ("content-type", AUTH_JSON),
        ],
        Some(
            serde_json::json!({ "replaces_credential_id": null })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "credential: {json}");
    json["token"].as_str().unwrap().to_owned()
}

/// 创建群房间并邀请两个 Agent 成员（Human creator + A + B），返回 Room ID。
async fn create_group_with_members(
    router: &axum::Router,
    human_token: &str,
    name: &str,
    member_a: &str,
    member_b: &str,
) -> String {
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {human_token}")),
        &[(KEY_HEADER, "agent-say-g-key"), ("content-type", AUTH_JSON)],
        Some(serde_json::json!({ "name": name }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "group: {json}");
    let room_id = json["room"]["id"].as_str().unwrap().to_owned();
    for (member_id, key) in [(member_a, "agent-say-m-a"), (member_b, "agent-say-m-b")] {
        let (status, json, _) = send(
            router,
            Method::POST,
            &format!("/v1/rooms/{room_id}/members"),
            Some(&format!("Bearer {human_token}")),
            &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
            Some(
                serde_json::json!({ "participant_id": member_id })
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "invite {member_id}: {json}");
    }
    room_id
}

/// agent 主动发消息（Backend Credential）。
async fn agent_say(
    router: &axum::Router,
    backend_token: &str,
    room_id: &str,
    body_value: &serde_json::Value,
    key: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        router,
        Method::POST,
        &format!("/v1/backend/rooms/{room_id}/messages"),
        Some(&format!("Bearer {backend_token}")),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(body_value.to_string().into_bytes()),
    )
    .await
}

/// 首次 say：201、actor 是认证 agent（kind=agent）、人类历史可视。
#[tokio::test]
async fn agent_say_created_201_actor_is_agent_human_history_visible() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "a1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, request_id) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "agent speaks" }),
        "say-1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(json["message"]["room_id"], room_id);
    assert_eq!(json["message"]["body"], "agent speaks");
    assert_eq!(json["message"]["actor"]["id"], agent_id);
    assert_eq!(json["message"]["actor"]["kind"], "agent");
    assert_eq!(json["message"]["actor"]["display_name"], "助手");
    assert_eq!(json["message"]["actor"]["status"], "active");
    assert_eq!(
        json["request_id"],
        request_id.as_deref().unwrap_or_default()
    );

    // 人类侧经既有房间 History 读到该条 Message（author = 该 agent）。
    let (status, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(status, StatusCode::OK);
    let agent_messages: Vec<_> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["actor"]["id"].as_str() == Some(agent_id.as_str()))
        .collect();
    assert_eq!(agent_messages.len(), 1);
    assert_eq!(agent_messages[0]["actor"]["kind"], "agent");
}

/// 同 tool-call-id（Idempotency-Key）重试 → 恰好一条：第二次 200 + 同一
/// Message，History 中该正文只有一条。
#[tokio::test]
async fn agent_say_same_key_replays_exactly_one_message() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "a2").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, first, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "exactly once" }),
        "say-exact",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let id = first["message"]["id"].as_str().unwrap().to_owned();

    let (status, second, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "exactly once" }),
        "say-exact",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["idempotency_replayed"], true);
    assert_eq!(
        second["message"]["id"], id,
        "同 key 重试必须返回同一 Message"
    );

    let (_, history, _) = get_history(&f.router, &f.token_a, &room_id, "").await;
    assert_eq!(
        history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["actor"]["id"].as_str() == Some(agent_id.as_str()))
            .count(),
        1,
        "恰好一条，不重复落账"
    );
}

/// 同 key 不同正文 → 409 idempotency_conflict（与人类路径同一裁决）。
#[tokio::test]
async fn agent_say_same_key_different_body_conflicts_409() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "a3").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, _, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "v1" }),
        "say-conflict",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, json, request_id) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "v2" }),
        "say-conflict",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    assert_eq!(
        json["error"]["request_id"],
        request_id.as_deref().unwrap_or_default()
    );
}

/// 非成员房间 say → 403 not_room_member（Room 存在但无 active Membership；
/// agent 已通过 BackendAuth 证明身份，是明确 ACL 拒绝）。
#[tokio::test]
async fn agent_say_non_member_room_rejected_403() {
    let f = fixture().await;
    let agent_a = create_agent(&f.router, &f.token_a, "AgentA", "a-k").await;
    // Agent A 的凭据用于建其专属 room；非成员测试用 B 的 token。
    let _token_a = create_backend_token(&f.router, &f.token_a, &agent_a, "ba").await;
    let room_a = create_room(&f.router, &f.token_a, &agent_a, "r-a").await;
    let agent_b = create_agent(&f.router, &f.token_a, "AgentB", "b-k").await;
    let token_b = create_backend_token(&f.router, &f.token_a, &agent_b, "bb").await;
    // room_a 只有 human+AgentA；AgentB 不是成员。
    let (status, json, request_id) = agent_say(
        &f.router,
        &token_b,
        &room_a,
        &serde_json::json!({ "body": "intrude" }),
        "say-intrude",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["error"]["code"], "not_room_member");
    assert_eq!(
        json["error"]["request_id"],
        request_id.as_deref().unwrap_or_default()
    );
    // 失败的尝试不留任何 Message 或幂等事实。
    let (_, history, _) = get_history(&f.router, &f.token_a, &room_a, "").await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 0);
}

/// unknown/bad-id 房间 → 404 room_not_found（与人类路径同一存在性保护）。
#[tokio::test]
async fn agent_say_unknown_room_404() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "a5").await;
    let ghost = uuid::Uuid::new_v4().to_string();

    let (status, json, _) = agent_say(
        &f.router,
        &token,
        &ghost,
        &serde_json::json!({ "body": "ghost" }),
        "say-ghost",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");
}

/// 缺少 Idempotency-Key → 400 idempotency_key_required；格式非法 → 400。
#[tokio::test]
async fn agent_say_requires_idempotency_key() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "a6").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token}")),
        &[("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "body": "no key" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_key_required");

    let (status, json, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "bad key" }),
        "bad key with spaces and comma, twice",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_idempotency_key");
}

/// 空正文 → 400 invalid_message_body；Body+attachments 都缺省 → 400
/// invalid_request（与人类路径同一正文裁决）。
#[tokio::test]
async fn agent_say_invalid_body_400() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "a-k").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "a7").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-k").await;

    let (status, json, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "" }),
        "say-empty",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_message_body");

    let (status, json, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({}),
        "say-none",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");
}

/// direct 房携带非空 mentions → 400 invalid_mention（与人类路径同一语义）。
#[tokio::test]
async fn agent_say_direct_room_mentions_rejected_400() {
    let f = fixture().await;
    let agent_a = create_agent(&f.router, &f.token_a, "AgentA", "a-k").await;
    let token_a = create_backend_token(&f.router, &f.token_a, &agent_a, "ba1").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_a, "r-a").await;

    let (status, json, _) = agent_say(
        &f.router,
        &token_a,
        &room_id,
        &serde_json::json!({ "body": "with mention", "mentions": [agent_a] }),
        "say-mention-direct",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_mention");
}

/// 群房间 + mention：A 的 say 经既有 Mention 管道生成 B 的 message.created
/// Event；B 经既有 /v1/backend/events（轮询）读到并可被唤醒（响应式事件
/// 路径零改动——冻结 007 语义的既有管道）。
#[tokio::test]
async fn agent_say_group_mention_wakes_b_through_existing_pipeline() {
    let f = fixture().await;
    let agent_a = create_agent(&f.router, &f.token_a, "AgentA", "a-k").await;
    let agent_b = create_agent(&f.router, &f.token_a, "AgentB", "b-k").await;
    let token_a = create_backend_token(&f.router, &f.token_a, &agent_a, "ga").await;
    let token_b = create_backend_token(&f.router, &f.token_a, &agent_b, "gb").await;
    let room_id =
        create_group_with_members(&f.router, &f.token_a, "Say Trio", &agent_a, &agent_b).await;

    let (status, json, _) = agent_say(
        &f.router,
        &token_a,
        &room_id,
        &serde_json::json!({ "body": "hey B", "mentions": [agent_b] }),
        "say-group",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["message"]["mentions"][0], agent_b);

    // B 经既有 backend events 轮询读到 message.created。
    let (status, page, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events?limit=50",
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let created: Vec<_> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"].as_str() == Some("message.created"))
        .collect();
    assert!(!created.is_empty(), "B 必须收到 message.created：{page}");

    // 作者 A 不自触发（冻结 007 语义：agent actor 不生成自己的 Event）。
    let (status, page_a, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events?limit=50",
        Some(&format!("Bearer {token_a}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page_a}");
    assert!(
        page_a["events"].as_array().unwrap().is_empty(),
        "作者 A 不得自触发 Event：{page_a}"
    );

    // 不带 mention 的普通群消息同样不唤醒成员 agent（与人类普通群消息同律）。
    let (status, json, _) = agent_say(
        &f.router,
        &token_a,
        &room_id,
        &serde_json::json!({ "body": "plain group message" }),
        "say-plain",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let (status, page_b2, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events?limit=50",
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page_b2}");
    assert_eq!(
        page_b2["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"].as_str() == Some("message.created"))
            .count(),
        created.len(),
        "普通（无 mention）say 不新增 B 的 Event——冻结语义零改动"
    );
}

// ---------------------------------------------------------------------------
// Backend agent 历史读取面（CORE_V1_AGENT_HISTORY.md，切片 B2 翻记录）
// ---------------------------------------------------------------------------

/// agent 经 BackendAuth 读取房间历史（Backend GET）。
async fn backend_get_history(
    router: &axum::Router,
    backend_token: &str,
    room_id: &str,
    query: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        router,
        Method::GET,
        &format!("/v1/backend/rooms/{room_id}/messages{query}"),
        Some(&format!("Bearer {backend_token}")),
        &[],
        None,
    )
    .await
}

/// 成员读历史：200、升序、作者投影正确（Human + Agent say 同页）、人类与
/// 后端两端点同构（同一 messages 形状 + next_before）。
#[tokio::test]
async fn backend_history_member_page_ascending_actor_projection() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "h-a").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "h-t").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "h-r").await;

    let (status, _, _) = send_message(&f.router, &f.token_a, &room_id, "human one", "h-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = send_message(&f.router, &f.token_a, &room_id, "human two", "h-2").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, json, _) = agent_say(
        &f.router,
        &token,
        &room_id,
        &serde_json::json!({ "body": "agent says" }),
        "h-say",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    let (status, json, _) = backend_get_history(&f.router, &token, &room_id, "").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let messages = json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "{json}");
    assert_eq!(messages[0]["body"], "human one");
    assert_eq!(messages[0]["actor"]["kind"], "human");
    assert_eq!(messages[1]["body"], "human two");
    assert_eq!(messages[2]["body"], "agent says");
    assert_eq!(messages[2]["actor"]["id"], agent_id);
    assert_eq!(messages[2]["actor"]["kind"], "agent");
    assert_eq!(json["next_before"], serde_json::Value::Null);
    assert!(json["request_id"].is_string());
}

/// 分页：首页 limit=2 取最新两条；before 游标翻更早页、与首页零重叠；
/// 与人类历史端点返回同一形状（同构）。
#[tokio::test]
async fn backend_history_pagination_no_overlap_older_pages() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "p-a").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "p-t").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "p-r").await;
    for i in 1..=5u32 {
        let (status, _, _) = send_message(
            &f.router,
            &f.token_a,
            &room_id,
            &format!("msg-{i}"),
            &format!("p-m{i}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, json, _) = backend_get_history(&f.router, &token, &room_id, "?limit=2").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let page1: Vec<String> = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page1.len(), 2, "limit=2 首页两条：{json}");
    assert_eq!(json["messages"][0]["body"], "msg-4");
    assert_eq!(json["messages"][1]["body"], "msg-5");
    let before = json["next_before"]
        .as_str()
        .expect("有更旧消息必有游标")
        .to_owned();

    let (status, json, _) = backend_get_history(
        &f.router,
        &token,
        &room_id,
        &format!("?limit=2&before={before}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let page2: Vec<String> = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page2.len(), 2, "{json}");
    assert_eq!(json["messages"][0]["body"], "msg-2");
    assert_eq!(json["messages"][1]["body"], "msg-3");
    assert!(
        page1.iter().all(|id| !page2.contains(id)),
        "前后两页零重叠：{page1:?} / {page2:?}"
    );
    let before2 = json["next_before"].as_str().expect("还有更旧").to_owned();

    let (status, json, _) = backend_get_history(
        &f.router,
        &token,
        &room_id,
        &format!("?limit=2&before={before2}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let page3: Vec<String> = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page3.len(), 1, "{json}");
    assert_eq!(json["messages"][0]["body"], "msg-1");
    assert_eq!(json["next_before"], serde_json::Value::Null);
    let all: Vec<&String> = page1
        .iter()
        .chain(page2.iter())
        .chain(page3.iter())
        .collect();
    assert_eq!(all.len(), 5, "三页合计恰 5 条不重不漏");
    for id in &all {
        assert_eq!(
            all.iter().filter(|x| *x == id).count(),
            1,
            "全量无重复：{all:?}"
        );
    }
}

/// 非成员读历史 → 403 not_room_member（对齐 say 端点 ACL）；bad-id/未知
/// Room → 404 room_not_found；401 认证语义同四端点。
#[tokio::test]
async fn backend_history_membership_403_and_401_and_404() {
    let f = fixture().await;
    let agent_a = create_agent(&f.router, &f.token_a, "AgentA", "m-a").await;
    let agent_b = create_agent(&f.router, &f.token_a, "AgentB", "m-b").await;
    let token_a = create_backend_token(&f.router, &f.token_a, &agent_a, "ma").await;
    let token_b = create_backend_token(&f.router, &f.token_a, &agent_b, "mb").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_a, "m-r").await;

    // B 不是 A 的 Direct Room 成员 → 403 not_room_member。
    let (status, json, _) = backend_get_history(&f.router, &token_b, &room_id, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["error"]["code"], "not_room_member");

    // bad-id / 未知 Room（成员 A）→ 404 room_not_found（不泄露存在性）。
    let (status, json, _) = backend_get_history(&f.router, &token_a, "not-a-uuid", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // 缺 Authorization → 401 authentication_required；坏 token → 401
    // invalid_credential（同四端点）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/backend/rooms/{room_id}/messages"),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_required");
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/backend/rooms/{room_id}/messages"),
        Some("Bearer clr_bc_not-a-real-token"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "invalid_credential");
}

/// 分页参数非法 → 400 invalid_request（limit 越界 / before 类型错），
/// 不泄露存在性（与人类路径同一裁决）。
#[tokio::test]
async fn backend_history_invalid_paging_400() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "助手", "q-a").await;
    let token = create_backend_token(&f.router, &f.token_a, &agent_id, "q-t").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "q-r").await;

    for query in ["?limit=0", "?limit=101", "?before=c1.be.ack", "?limit=abc"] {
        let (status, json, _) = backend_get_history(&f.router, &token, &room_id, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "query {query}: {json}");
        assert_eq!(json["error"]["code"], "invalid_request");
    }
}
