//! Group Room Profile HTTP 契约测试（CORE_V1_ROOM_PROFILE.md §3–§5）。
//!
//! 在真实临时 SQLite + 全部 migration 上走完整路由器：群主写、成员读、
//! 幂等重放 / 冲突、owner 转让、错误矩阵（403/404/400/409/401）、
//! `GET /v1/rooms/{id}` 的 `profile` 字段、`GET /v1/backend/rooms/{id}/messages`
//! 的顶层 `room` 头与分页不变。不直读 SQLite 作为成功证据（只在断言「失败
//! 路径没有多出版本」时读计数）。

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
use crate::adapters::sqlite::test_relations::{
    seed_accepted_relation, seed_accepted_relation_for_token,
};
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
    pool: SqlitePool,
    alice: String,
    alice_token: String,
    bob: String,
    bob_token: String,
    carol_token: String,
}

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
    .expect("insert human");
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
    let (bob, bob_token) = add_human(&pool, &identity, "Bob").await;
    let (_carol, carol_token) = add_human(&pool, &identity, "Carol").await;
    let alice_token = a.token.as_str().to_owned();
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
        pool,
        alice: a.participant.id.as_str().to_owned(),
        alice_token,
        bob,
        bob_token,
        carol_token,
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

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn create_agent(router: &axum::Router, token: &str, name: &str, key: &str) -> String {
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/agents",
        Some(&bearer(token)),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "display_name": name })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

async fn create_group(router: &axum::Router, token: &str, name: &str, key: &str) -> String {
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/groups",
        Some(&bearer(token)),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(serde_json::json!({ "name": name }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    json["room"]["id"].as_str().unwrap().to_owned()
}

async fn invite(router: &axum::Router, token: &str, room: &str, target: &str, key: &str) {
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room}/members"),
        Some(&bearer(token)),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "participant_id": target })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite: {json}");
}

fn put_body(base: i64, description: &str) -> Vec<u8> {
    serde_json::json!({ "base_version": base, "description": description })
        .to_string()
        .into_bytes()
}

async fn put_profile(
    router: &axum::Router,
    token: &str,
    room: &str,
    key: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut headers: Vec<(&str, &str)> = vec![("content-type", AUTH_JSON)];
    if let Some(key) = key {
        headers.push((KEY_HEADER, key));
    }
    send(
        router,
        Method::PUT,
        &format!("/v1/rooms/{room}/profile"),
        Some(&bearer(token)),
        &headers,
        Some(body),
    )
    .await
}

async fn get_room(
    router: &axum::Router,
    token: &str,
    room: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, json, _) = send(
        router,
        Method::GET,
        &format!("/v1/rooms/{room}"),
        Some(&bearer(token)),
        &[],
        None,
    )
    .await;
    (status, json)
}

async fn backend_token(router: &axum::Router, human_token: &str, agent: &str, tag: &str) -> String {
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent}/backend-bindings"),
        Some(&bearer(human_token)),
        &[
            (KEY_HEADER, &format!("rp-b-{tag}")),
            ("content-type", AUTH_JSON),
        ],
        Some(
            serde_json::json!({ "label": "rp" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let binding = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent}/backend-bindings/{binding}/credentials"),
        Some(&bearer(human_token)),
        &[
            (KEY_HEADER, &format!("rp-c-{tag}")),
            ("content-type", AUTH_JSON),
        ],
        Some(
            serde_json::json!({ "replaces_credential_id": null })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    json["token"].as_str().unwrap().to_owned()
}

async fn backend_history(
    router: &axum::Router,
    token: &str,
    room: &str,
    query: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, json, _) = send(
        router,
        Method::GET,
        &format!("/v1/backend/rooms/{room}/messages{query}"),
        Some(&bearer(token)),
        &[],
        None,
    )
    .await;
    (status, json)
}

async fn send_message(router: &axum::Router, token: &str, room: &str, body: &str, key: &str) {
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&bearer(token)),
        &[(KEY_HEADER, key), ("content-type", AUTH_JSON)],
        Some(serde_json::json!({ "body": body }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
}

async fn profile_version_count(pool: &SqlitePool, room: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM room_profile_versions WHERE room_id = ?")
        .bind(room)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ============================================================
// 1. 群主写 / 成员读 / 重放 / 列表形状
// ============================================================

#[tokio::test]
async fn owner_write_read_replay_and_list_keeps_room_view() {
    let f = fixture().await;
    let room = create_group(&f.router, &f.alice_token, "研发群", "g-1").await;

    // 无简介 → profile: null，RoomView 本身不多字段。
    let (status, json) = get_room(&f.router, &f.alice_token, &room).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json["profile"].is_null());
    assert_eq!(json["room"]["name"], "研发群");
    assert!(json["room"].get("profile").is_none());
    assert_eq!(
        json["room"].as_object().unwrap().keys().count(),
        5,
        "RoomView 一字未动：{json}"
    );

    let text = "第一行\n第二行\r\n\t缩进 🙂 café";
    let (status, json, request_id) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-1"),
        put_body(0, text),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["description"], text);
    assert_eq!(json["profile"]["version"], 1);
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(
        json["request_id"],
        request_id.as_deref().unwrap_or_default()
    );
    let updated_at = json["profile"]["updated_at"].as_str().unwrap();
    assert!(
        updated_at.contains('T') && updated_at.len() >= 20,
        "{updated_at}"
    );

    let (status, json) = get_room(&f.router, &f.alice_token, &room).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["profile"]["version"], 1);
    assert_eq!(json["profile"]["description"], text);

    // 同 key 同摘要 → 200 重放，不追加版本。
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-1"),
        put_body(0, text),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["profile"]["version"], 1);
    assert_eq!(profile_version_count(&f.pool, &room).await, 1);

    // 新 key + base 1 → 版本 2。
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-2"),
        put_body(1, "第二版简介"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["version"], 2);

    // 列表端点不带简介，RoomView 无这一格（v1.1 的负空间）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&bearer(&f.alice_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let item = json["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["room"]["id"] == room)
        .expect("room in list");
    assert!(item.get("profile").is_none());
    assert!(item["room"].get("profile").is_none());
    assert_eq!(item["room"].as_object().unwrap().keys().count(), 5);
}

// ============================================================
// 2. 权限矩阵与错误码
// ============================================================

#[tokio::test]
async fn permission_and_error_matrix_stays_fail_closed() {
    let f = fixture().await;
    let room = create_group(&f.router, &f.alice_token, "权限群", "g-perm").await;
    seed_accepted_relation_for_token(&f.pool, &f.alice_token, &f.bob).await;
    invite(&f.router, &f.alice_token, &room, &f.bob, "inv-bob").await;
    let agent = create_agent(&f.router, &f.alice_token, "助手", "a-perm").await;
    invite(&f.router, &f.alice_token, &room, &agent, "inv-agent").await;

    // 非 owner 的 active 成员 → 403 not_room_owner。
    let (status, json, _) = put_profile(
        &f.router,
        &f.bob_token,
        &room,
        Some("rp-b"),
        put_body(0, "Bob 写"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["error"]["code"], "not_room_owner");

    // 非成员 / 未知 / bad-id → 404 room_not_found。
    for target in [
        room.clone(),
        Uuid::new_v4().to_string(),
        "not-a-uuid".to_string(),
    ] {
        let token = if target == room {
            &f.carol_token
        } else {
            &f.alice_token
        };
        let (status, json, _) = put_profile(
            &f.router,
            token,
            &target,
            Some("rp-x"),
            put_body(0, "探存在性"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{target}: {json}");
        assert_eq!(json["error"]["code"], "room_not_found");
    }

    // direct 房 → 400 invalid_room_kind（Alice 是 owner）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&bearer(&f.alice_token)),
        &[(KEY_HEADER, "d-1"), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "agent_id": agent })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let direct = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &direct,
        Some("rp-d"),
        put_body(0, "私聊"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_room_kind");

    // 群主首次写入 → 版本 1。
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-ok"),
        put_body(0, "第一版"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    // base_version 过期 → 409 profile_version_conflict。
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-stale"),
        put_body(0, "过期"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "profile_version_conflict");

    // 同 key 不同摘要 → 409 idempotency_conflict。
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-ok"),
        put_body(0, "第一版改过"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // 缺 key / key 形状错。
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        None,
        put_body(1, "无 key"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_key_required");
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("a,b"),
        put_body(1, "坏 key"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_idempotency_key");

    // 正文边界：空 / 纯空白 / NUL / 2001 字节。
    for (index, (description, why)) in [
        ("".to_string(), "empty"),
        (" \n\t\r ".to_string(), "blank"),
        ("a\u{0}b".to_string(), "nul"),
        ("a".repeat(2_001), "too long"),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, json, _) = put_profile(
            &f.router,
            &f.alice_token,
            &room,
            Some(&format!("rp-desc-{index}")),
            put_body(1, &description),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {json}");
        assert_eq!(
            json["error"]["code"], "invalid_profile_description",
            "{why}"
        );
    }

    // 恰好 2000 字节合法（边界另一侧）。
    let exact = "é".repeat(1_000);
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("rp-exact"),
        put_body(1, &exact),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["version"], 2);

    // 形状错 / 缺字段 / 负 base_version / 未知字段。
    for body in [
        serde_json::json!({ "base_version": 2 }).to_string(),
        serde_json::json!({ "base_version": 2, "description": null }).to_string(),
        serde_json::json!({ "base_version": -1, "description": "x" }).to_string(),
        serde_json::json!({ "base_version": 2, "description": "x", "extra": 1 }).to_string(),
        "not json".to_string(),
    ] {
        let (status, json, _) = put_profile(
            &f.router,
            &f.alice_token,
            &room,
            Some(&format!("rp-shape-{}", Uuid::new_v4())),
            body.clone().into_bytes(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {json}");
        assert_eq!(json["error"]["code"], "invalid_request", "{body}");
    }

    // 401：无凭据 / Agent 凭据都不行（Human Session 唯一认证面）。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room}/profile"),
        None,
        &[(KEY_HEADER, "rp-anon"), ("content-type", AUTH_JSON)],
        Some(put_body(2, "匿名")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_required");

    // 失败路径没有多出版本：v1（第一版）+ v2（恰好 2000 字节）。
    assert_eq!(profile_version_count(&f.pool, &room).await, 2);
}

// ============================================================
// 3. 群主转让后新群主可写、旧群主 403
// ============================================================

#[tokio::test]
async fn owner_transfer_moves_write_authority() {
    let f = fixture().await;
    let room = create_group(&f.router, &f.alice_token, "转让群", "g-tr").await;
    seed_accepted_relation_for_token(&f.pool, &f.alice_token, &f.bob).await;
    invite(&f.router, &f.alice_token, &room, &f.bob, "tr-bob").await;

    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("tr-1"),
        put_body(0, "旧群主"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/owner-transfer"),
        Some(&bearer(&f.alice_token)),
        &[(KEY_HEADER, "tr-transfer"), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "participant_id": f.bob })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "transfer: {json}");
    assert_eq!(json["membership"]["role"], "owner");

    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("tr-old"),
        put_body(1, "旧群主再写"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["error"]["code"], "not_room_owner");

    let (status, json, _) = put_profile(
        &f.router,
        &f.bob_token,
        &room,
        Some("tr-new"),
        put_body(1, "新群主写"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["version"], 2);
    assert_eq!(json["profile"]["description"], "新群主写");

    // 旧群主仍是成员：读得到新版本。
    let (status, json) = get_room(&f.router, &f.alice_token, &room).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["profile"]["version"], 2);
    assert_eq!(json["profile"]["description"], "新群主写");
}

// ============================================================
// 4. 单房历史回执的房间头 + 既有分页零变化
// ============================================================

#[tokio::test]
async fn backend_history_carries_room_header_and_keeps_pagination() {
    let f = fixture().await;
    let room = create_group(&f.router, &f.alice_token, "研发群", "g-hist").await;
    let agent = create_agent(&f.router, &f.alice_token, "助手", "a-hist").await;
    invite(&f.router, &f.alice_token, &room, &agent, "hist-agent").await;
    let token = backend_token(&f.router, &f.alice_token, &agent, "hist").await;

    for i in 1..=5 {
        send_message(
            &f.router,
            &f.alice_token,
            &room,
            &format!("msg-{i}"),
            &format!("h-{i}"),
        )
        .await;
    }
    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("h-p1"),
        put_body(0, "群简介\n第二行 🙂"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let (status, page1) = backend_history(&f.router, &token, &room, "?limit=2").await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["messages"].as_array().unwrap().len(), 2);
    assert_eq!(page1["room"]["id"], room);
    assert_eq!(page1["room"]["kind"], "group");
    assert_eq!(page1["room"]["name"], "研发群");
    assert_eq!(page1["room"]["profile"]["version"], 1);
    assert_eq!(page1["room"]["profile"]["description"], "群简介\n第二行 🙂");
    assert!(page1["room"]["profile"].get("updated_at").is_none());
    let next = page1["next_before"]
        .as_str()
        .expect("older page")
        .to_owned();
    assert_eq!(page1["messages"][0]["body"], "msg-4");
    assert_eq!(page1["messages"][1]["body"], "msg-5");

    let (status, page2) =
        backend_history(&f.router, &token, &room, &format!("?limit=2&before={next}")).await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["room"], page1["room"], "每一页都带同一房间头");
    assert_eq!(page2["messages"].as_array().unwrap().len(), 2);
    assert_eq!(page2["messages"][0]["body"], "msg-2");
    assert_eq!(page2["messages"][1]["body"], "msg-3");
    let next2 = page2["next_before"].as_str().unwrap().to_owned();
    let (status, page3) = backend_history(
        &f.router,
        &token,
        &room,
        &format!("?limit=2&before={next2}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page3["messages"].as_array().unwrap().len(), 1);
    assert_eq!(page3["messages"][0]["body"], "msg-1");
    assert!(page3["next_before"].is_null());

    // 三页无重复、无跳过。
    let mut ids: Vec<&str> = Vec::new();
    for page in [&page1, &page2, &page3] {
        for message in page["messages"].as_array().unwrap() {
            ids.push(message["id"].as_str().unwrap());
        }
    }
    assert_eq!(ids.len(), 5);
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 5, "分页零重叠：{ids:?}");

    // 人类历史响应不带 room 头（附加字段只在 Backend 面）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/messages?limit=2"),
        Some(&bearer(&f.alice_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.get("room").is_none());
    assert_eq!(json["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn history_header_is_null_without_profile_and_for_direct_rooms() {
    let f = fixture().await;
    // 有成员但无简介的 group → room.profile null。
    let group = create_group(&f.router, &f.alice_token, "空的群", "g-empty").await;
    let agent_a = create_agent(&f.router, &f.alice_token, "A", "a-empty").await;
    invite(&f.router, &f.alice_token, &group, &agent_a, "empty-a").await;
    let token_a = backend_token(&f.router, &f.alice_token, &agent_a, "empty").await;
    let (status, json) = backend_history(&f.router, &token_a, &group, "").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["room"]["kind"], "group");
    assert!(json["room"]["profile"].is_null());
    assert_eq!(json["messages"], serde_json::json!([]));

    // 无简介不影响 `room.name`。
    assert_eq!(json["room"]["name"], "空的群");

    // direct 房 → room.kind=direct + profile null。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&bearer(&f.alice_token)),
        &[(KEY_HEADER, "d-hist"), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "agent_id": agent_a })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let direct = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json) = backend_history(&f.router, &token_a, &direct, "").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["room"]["id"], direct);
    assert_eq!(json["room"]["kind"], "direct");
    assert!(json["room"]["name"].is_null());
    assert!(json["room"]["profile"].is_null());

    // 非成员仍是 403 not_room_member（房间头不改变准入）。
    let agent_b = create_agent(&f.router, &f.alice_token, "B", "a-out").await;
    let token_b = backend_token(&f.router, &f.alice_token, &agent_b, "out").await;
    let (status, json) = backend_history(&f.router, &token_b, &group, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["error"]["code"], "not_room_member");

    // 未知房仍是 404。
    let (status, json) =
        backend_history(&f.router, &token_a, &Uuid::new_v4().to_string(), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");
}

// ============================================================
// 5. Agent owner 的群：Human 成员不能替它写
// ============================================================

#[tokio::test]
async fn an_agent_owned_group_cannot_be_written_by_a_human_member() {
    let f = fixture().await;
    // Agent 自己建房（owner = Agent），再把它与 Alice 的关系作为邀请资格。
    let agent = create_agent(&f.router, &f.alice_token, "群主 Agent", "a-owner").await;
    let token = backend_token(&f.router, &f.alice_token, &agent, "owner").await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/backend/groups",
        Some(&bearer(&token)),
        &[(KEY_HEADER, "bg-1"), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "name": "Agent 的群" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let room = json["room"]["id"].as_str().unwrap().to_owned();

    seed_accepted_relation(&f.pool, &agent, &f.alice).await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{room}/members"),
        Some(&bearer(&token)),
        &[(KEY_HEADER, "bg-inv"), ("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "participant_id": f.alice.as_str() })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    let (status, json, _) = put_profile(
        &f.router,
        &f.alice_token,
        &room,
        Some("bg-p"),
        put_body(0, "人类成员想写"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(json["error"]["code"], "not_room_owner");
    assert_eq!(profile_version_count(&f.pool, &room).await, 0);
}
