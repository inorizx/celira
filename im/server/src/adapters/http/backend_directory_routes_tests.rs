//! Agent 通讯录端点 HTTP 契约测试（CORE_V1_AGENT_DIRECTORY.md，切片 B1）：
//! BackendAuth 认证（401 语义对齐四端点）、active membership 可见性（非成员
//! 房间不可见）、每房预览/显示名/最近活动/`since` 增量计数、机械排序（有
//! 消息房在前、无消息房按创建时间殿后）、非法 `since` → 400。断言全走公共
//! HTTP 协议（GET /v1/backend/rooms），不直读 SQLite 冒充证据。

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
    agent_a_token: String,
    agent_b_token: String,
    direct_room: String,
    group_room: String,
    empty_room: String,
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session（既有测试同款 fixture
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
        Some("a-1"),
        Some(&serde_json::json!({"display_name": "Agent A"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create agent A");
    let agent_a = json["agent"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/agents",
        &human_token,
        Some("a-2"),
        Some(&serde_json::json!({"display_name": "Agent B"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create agent B");
    let agent_b = json["agent"]["id"].as_str().unwrap().to_owned();

    let agent_a_token = credential_for(&router, &human_token, &agent_a, "b-a").await;
    let agent_b_token = credential_for(&router, &human_token, &agent_b, "b-b").await;

    // Direct Room（Human + A，带标题）——A 是 active member。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/direct-rooms",
        &human_token,
        Some("d-1"),
        Some(&serde_json::json!({ "agent_id": agent_a.clone()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create direct room");
    let direct_room = json["room"]["id"].as_str().unwrap().to_owned();

    // 空 Direct Room（Human + **B**）——无消息，机械排序殿后。
    // 批次二「一间房 = 一段关系」：**同一对只有一间私人房**（再要一次是 replay
    // 回原房，不是 201），所以第二间房必须是**另一段关系**——这里用 B 那条。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/direct-rooms",
        &human_token,
        Some("d-2"),
        Some(&serde_json::json!({"agent_id": agent_b.clone()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create empty direct room");
    let empty_room = json["room"]["id"].as_str().unwrap().to_owned();

    // Group Room（Human + A + B）。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/groups",
        &human_token,
        Some("g-1"),
        Some(&serde_json::json!({"name": "项目群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create group room");
    let group_room = json["room"]["id"].as_str().unwrap().to_owned();
    for (key, agent) in [("i-1", &agent_a), ("i-2", &agent_b)] {
        let (status, json, _) = request_json(
            &router,
            Method::POST,
            &format!("/v1/rooms/{group_room}/members"),
            &human_token,
            Some(key),
            Some(&serde_json::json!({"participant_id": agent})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "invite {agent}: {json}");
    }

    Fixture {
        _dir: dir,
        router,
        human_token,
        agent_a_token,
        agent_b_token,
        direct_room,
        group_room,
        empty_room,
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
        Some(&serde_json::json!({"label": "directory e2e"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create binding {agent_id}");
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
    assert_eq!(status, StatusCode::CREATED, "issue credential {agent_id}");
    json["token"].as_str().unwrap().to_owned()
}

async fn send(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> (StatusCode, serde_json::Value, Option<String>) {
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
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    eprintln!("DEBUG RAW {method} {path}: status={status} {json}");
    (status, json, request_id)
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
    send(
        router,
        method,
        path,
        Some(&format!("Bearer {token}")),
        &headers,
        json_body.map(|v| v.to_string().into_bytes()),
    )
    .await
}

/// 目录查询（GET /v1/backend/rooms；token 可空 = 无 Authorization）。
async fn directory(
    router: &axum::Router,
    token: Option<&str>,
    since: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let path = match since {
        Some(s) => format!("/v1/backend/rooms?since={s}"),
        None => "/v1/backend/rooms".to_string(),
    };
    let (status, json, _) = send(
        router,
        Method::GET,
        &path,
        token.map(|t| format!("Bearer {t}")).as_deref(),
        &[],
        None,
    )
    .await;
    (status, json)
}

fn rooms(json: &serde_json::Value) -> Vec<serde_json::Value> {
    json["rooms"].as_array().cloned().unwrap_or_default()
}

fn find_room<'a>(json: &'a serde_json::Value, room_id: &str) -> &'a serde_json::Value {
    json["rooms"]
        .as_array()
        .and_then(|list| list.iter().find(|r| r["room_id"].as_str() == Some(room_id)))
        .unwrap_or_else(|| panic!("room {room_id} must be in directory: {json}"))
}

/// 人类向房间发送普通 Message（公开 API）。
async fn human_message(
    router: &axum::Router,
    human_token: &str,
    room: &str,
    key: &str,
    body: &str,
) {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        human_token,
        Some(key),
        Some(&serde_json::json!({"body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "human send {body}: {json}");
}

/// agent 向房间主动发消息（切片 A 端点）。
async fn agent_say(router: &axum::Router, token: &str, room: &str, key: &str, body: &str) {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/backend/rooms/{room}/messages"),
        token,
        Some(key),
        Some(&serde_json::json!({"body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent say {body}: {json}");
}

#[tokio::test]
async fn directory_lists_member_rooms_with_preview_and_since_counts() {
    let f = fixture().await;
    human_message(&f.router, &f.human_token, &f.direct_room, "h-1", "你好").await;
    human_message(&f.router, &f.human_token, &f.direct_room, "h-2", "在吗").await;
    agent_say(&f.router, &f.agent_a_token, &f.direct_room, "a-1", "我在").await;
    human_message(&f.router, &f.human_token, &f.group_room, "h-3", "群消息").await;

    // A 的目录：**2 房**（Direct + Group）。批次二「一间房 = 一段关系」：空房是与
    // **B** 的另一段关系，A 不是它的成员 ⇒ 看不到（这正是"非成员不可见"）。
    let (status, json) = directory(&f.router, Some(&f.agent_a_token), Some("0")).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let list = rooms(&json);
    assert_eq!(list.len(), 2, "A 应看到自己的 2 个房间：{json}");

    // Direct：**没有标题**（批次二）⇒ 显示名回落到对方（Human Alice）的名字；
    // 预览 = 最近一条（A 的 say「我在」，按 sequence 取最新，不依赖时间）；
    // since=0 增量计数 = 2（人类两条，A 自己的 say 不计入积压）。
    let direct = find_room(&json, &f.direct_room);
    assert_eq!(direct["kind"].as_str(), Some("direct"));
    assert_eq!(direct["display_name"].as_str(), Some("Alice"));
    assert_eq!(direct["preview"].as_str(), Some("我在"));
    assert!(
        direct["last_activity_at"].is_string(),
        "last_activity_at 非空"
    );
    assert_eq!(direct["since_count"].as_u64(), Some(2));

    // Group：显示名 = 群名；预览 = 群消息；since=0 计数 = 1。
    let group = find_room(&json, &f.group_room);
    assert_eq!(group["kind"].as_str(), Some("group"));
    assert_eq!(group["display_name"].as_str(), Some("项目群"));
    assert_eq!(group["preview"].as_str(), Some("群消息"));
    assert_eq!(group["since_count"].as_u64(), Some(1));

    // A 看不到 B 那间空房（不是它的成员）。
    assert!(
        list.iter()
            .all(|r| r["room_id"].as_str() != Some(f.empty_room.as_str())),
        "非成员房间不得出现：{json}"
    );
}

#[tokio::test]
async fn directory_non_member_rooms_are_invisible() {
    let f = fixture().await;
    human_message(&f.router, &f.human_token, &f.group_room, "h-1", "群消息").await;

    // B 属于 Group 与**自己那间空房**（批次二后空房是 Human+B 的关系），
    // 但不属于 Human+A 的 Direct ⇒ 目录 2 房且不含 A 那间。
    let (status, json) = directory(&f.router, Some(&f.agent_b_token), None).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let list = rooms(&json);
    assert_eq!(list.len(), 2, "B 只能看到自己的房间：{json}");
    assert!(
        list.iter()
            .any(|r| r["room_id"].as_str() == Some(f.group_room.as_str())),
        "B 应在群里：{json}"
    );
    assert!(
        list.iter()
            .any(|r| r["room_id"].as_str() == Some(f.empty_room.as_str())),
        "B 应在自己那间房里：{json}"
    );
    assert!(
        list.iter()
            .all(|r| r["room_id"].as_str() != Some(f.direct_room.as_str())),
        "B 不得看到 A 那间房：{json}"
    );
    // 无 since → 无增量基线 → 计数恒 0。
    let group = find_room(&json, &f.group_room);
    assert_eq!(group["since_count"].as_u64(), Some(0));
    // 空房（B 那间）：无消息 ⇒ 预览 null、无活动时间；有消息的群排它前面。
    let empty = find_room(&json, &f.empty_room);
    assert_eq!(empty["preview"].as_str(), None);
    assert_eq!(empty["last_activity_at"].as_str(), None);
    assert_eq!(empty["since_count"].as_u64(), Some(0));
    assert_eq!(
        list.last().unwrap()["room_id"].as_str(),
        Some(f.empty_room.as_str()),
        "有消息的房间排在无消息房之前：{json}"
    );
}

#[tokio::test]
async fn directory_since_filters_incremental_counts() {
    let f = fixture().await;
    human_message(&f.router, &f.human_token, &f.direct_room, "h-1", "你好").await;

    // 远未来基线：该时间之后无消息 → 计数 0。
    let (status, json) = directory(&f.router, Some(&f.agent_a_token), Some("99999999999999")).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let direct = find_room(&json, &f.direct_room);
    assert_eq!(direct["since_count"].as_u64(), Some(0));
    assert_eq!(
        direct["preview"].as_str(),
        Some("你好"),
        "预览不受 since 影响"
    );

    // 负数 / 非数字 since → 400 invalid_request。
    for bad in ["-1", "abc", "1.5", ""] {
        let (status, json) = directory(&f.router, Some(&f.agent_a_token), Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "since={bad:?}: {json}");
        assert_eq!(json["error"]["code"].as_str(), Some("invalid_request"));
    }
}

#[tokio::test]
async fn directory_auth_401_aligned_with_four_endpoints() {
    let f = fixture().await;
    // 缺 Authorization → 401 authentication_required。
    let (status, json) = directory(&f.router, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {json}");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("authentication_required")
    );
    // 未知 token → 401 invalid_credential（同一错误文案，不形成存在性 oracle）。
    let (status, json) = directory(&f.router, Some("clr_bc_not-a-real-token"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_credential"));
}
