//! 提醒档（attention）端点 HTTP 契约测试（契约 `CORE_V1_IM_CONTRACT.md` §9.1，
//! 窄解冻切片 1a）：两条**同形**写端点的认证面、准入与"只有群房有这一档"、
//! body 形状与闭集取值，以及两条列表的 `attention` 投影（缺行 = `all`）。
//!
//! 断言全走公共 HTTP 协议，不直读 SQLite。迁移守卫那一半（只 group 房、成员
//! active、行身份不可改写）的权威证据在 `adapters/sqlite/room_attention_migration_tests.rs`
//! ——本文件只判端点契约，不重复判库里的守卫形状。
//!
//! ⚠ **本切片不判"通知发不发"**：那是 1b，唯一改变"谁被叫醒"的改动；本文件
//! 没有任何一条断言依赖门铃 / WS 行为。

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
    second_human_token: String,
    agent_token: String,
    /// Alice（owner）+ Agent A（member）的 group 房。
    group_room: String,
    /// Alice + Agent A 的 `direct` 房（两人房：没有提醒档）。
    direct_room: String,
    /// Alice + Bob 的 `human_direct` 房（两人房：没有提醒档）。
    human_direct_room: String,
    /// 只有 Alice 的 group 房（Agent A 不是成员）。
    solo_group_room: String,
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
    let (human_b, second_human_token) = add_human(&pool, &identity, "Bob").await;
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
    let human_a = a.participant.id.as_str().to_owned();

    // Agent A（Alice 拥有）+ 它的 Backend token。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/agents",
        &human_token,
        Some("at-a-1"),
        Some(&serde_json::json!({"display_name": "Agent A"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create agent A: {json}");
    let agent_a = json["agent"]["id"].as_str().unwrap().to_owned();
    let agent_token = credential_for(&router, &human_token, &agent_a).await;

    // group 房：Alice owner，随后把 A 拉进来。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/groups",
        &human_token,
        Some("at-g-1"),
        Some(&serde_json::json!({"name": "有提醒档的群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create group: {json}");
    let group_room = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        &format!("/v1/rooms/{group_room}/members"),
        &human_token,
        Some("at-g-1-invite"),
        Some(&serde_json::json!({"participant_id": agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite agent A: {json}");

    // 只有 Alice 的群（A 不是成员）。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/groups",
        &human_token,
        Some("at-g-2"),
        Some(&serde_json::json!({"name": "只有人的群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create solo group: {json}");
    let solo_group_room = json["room"]["id"].as_str().unwrap().to_owned();

    // direct 房（两人房）。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/direct-rooms",
        &human_token,
        Some("at-d-1"),
        Some(&serde_json::json!({"agent_id": agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create direct room: {json}");
    let direct_room = json["room"]["id"].as_str().unwrap().to_owned();

    // human_direct 房（两人房）：先建 accepted 关系。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/contacts/requests",
        &human_token,
        Some("at-c-1"),
        Some(&serde_json::json!({"participant_id": human_b})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "contact request: {json}");
    let (status, json) = send(
        &router,
        Method::POST,
        &format!("/v1/contacts/{human_a}/accept"),
        Some(&format!("Bearer {second_human_token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "accept contact: {json}");
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/human-direct-rooms",
        &human_token,
        Some("at-hd-1"),
        Some(&serde_json::json!({"human_id": human_b})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create human_direct: {json}");
    let human_direct_room = json["room"]["id"].as_str().unwrap().to_owned();

    // Bob 也进那个群（"另一位 Human 在同一间房"那条断言的前提）。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        &format!("/v1/rooms/{group_room}/members"),
        &human_token,
        Some("at-g-1-invite-b"),
        Some(&serde_json::json!({"participant_id": human_b})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite Bob: {json}");

    Fixture {
        _dir: dir,
        router,
        human_token,
        second_human_token,
        agent_token,
        group_room,
        direct_room,
        human_direct_room,
        solo_group_room,
    }
}

/// 创建 Binding + 签发 Credential，返回 backend token（公开 API 全链）。
async fn credential_for(router: &axum::Router, human_token: &str, agent_id: &str) -> String {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        human_token,
        Some("at-b-1"),
        Some(&serde_json::json!({"label": "attention e2e"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        human_token,
        Some("at-b-1-c"),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "issue credential: {json}");
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
    let _ = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json)
}

/// 走 Human 侧写端点（`/v1/rooms/{room}/attention`）。
async fn put_attention(
    router: &axum::Router,
    token: Option<&str>,
    room: &str,
    body: Option<&serde_json::Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    put_attention_at(router, token, "/v1/rooms/", room, body, extra).await
}

/// 走 Agent 侧写端点（`/v1/backend/rooms/{room}/attention`）。
async fn put_backend_attention(
    router: &axum::Router,
    token: Option<&str>,
    room: &str,
    body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    put_attention_at(router, token, "/v1/backend/rooms/", room, body, &[]).await
}

async fn put_attention_at(
    router: &axum::Router,
    token: Option<&str>,
    prefix: &str,
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
        &format!("{prefix}{room}/attention"),
        token.map(|t| format!("Bearer {t}")).as_deref(),
        &headers,
        body.map(|v| v.to_string().into_bytes()),
    )
    .await
}

/// 取 `GET /v1/rooms`（Human）里某一行的 `attention`：Human 侧的行形状是
/// `{room: {id, ...}, ...}`。
async fn human_attention(router: &axum::Router, token: &str, room: &str) -> serde_json::Value {
    let (status, json) = send(
        router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "human rooms: {json}");
    pick(&json, |row| row["room"]["id"].as_str(), room)
}

/// 取 `GET /v1/backend/rooms`（Agent）里某一行的 `attention`：目录的行形状是
/// 扁平的 `{room_id, ...}`。
async fn backend_attention(router: &axum::Router, token: &str, room: &str) -> serde_json::Value {
    let (status, json) = send(
        router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "backend rooms: {json}");
    pick(&json, |row| row["room_id"].as_str(), room)
}

/// 从一份列表响应里取出那间房那一行的 `attention`（两处行形状不同，判据同一条：
/// 那一格**必须存在**——它是投影，不是"有时才出现的字段"）。
fn pick(
    json: &serde_json::Value,
    id_of: impl Fn(&serde_json::Value) -> Option<&str>,
    room: &str,
) -> serde_json::Value {
    let row = json["rooms"]
        .as_array()
        .and_then(|rooms| rooms.iter().find(|row| id_of(row) == Some(room)))
        .unwrap_or_else(|| panic!("room {room} must be listed: {json}"));
    assert!(
        row.get("attention").is_some(),
        "每一行都必须带 attention: {row}"
    );
    row["attention"].clone()
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

/// 起点与往返：**缺行 = `all`**（两份列表都这么投影，两人房也一样）；
/// 写 `mentions` 之后两份列表都变；同值重写是 200（幂等，**不需要
/// Idempotency-Key**）；写回 `all` 之后恢复。
#[tokio::test]
async fn attention_defaults_to_all_and_a_group_member_can_set_it() {
    let f = fixture().await;

    // 起点：一条行都没建过 ⇒ 全部投影成 all（含两人房与"我还没设过的群"）。
    for room in [
        &f.group_room,
        &f.direct_room,
        &f.human_direct_room,
        &f.solo_group_room,
    ] {
        assert_eq!(
            human_attention(&f.router, &f.human_token, room)
                .await
                .as_str(),
            Some("all"),
            "缺行必须投影成 all：{room}"
        );
    }

    // 写 mentions：200，回写后的权威取值；**不带 Idempotency-Key**。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "set mentions: {json}");
    assert_eq!(json["attention"].as_str(), Some("mentions"));
    assert!(
        json.get("idempotency_replayed").is_none(),
        "状态写没有幂等键，响应里也不该出现重放标记: {json}"
    );
    assert_eq!(
        human_attention(&f.router, &f.human_token, &f.group_room)
            .await
            .as_str(),
        Some("mentions"),
        "列表投影必须读到刚写下的档"
    );

    // 同值重写：仍是 200 同一个形状（天然幂等，不改别人的行、不产生事件）。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "same value rewrite: {json}");
    assert_eq!(json["attention"].as_str(), Some("mentions"));

    // 写回 all。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "all"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "set all: {json}");
    assert_eq!(json["attention"].as_str(), Some("all"));
    assert_eq!(
        human_attention(&f.router, &f.human_token, &f.group_room)
            .await
            .as_str(),
        Some("all")
    );

    // 房名/其他人的行不受影响：另一位 Human 在同一间房里仍是缺行（all）。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        human_attention(&f.router, &f.second_human_token, &f.group_room)
            .await
            .as_str(),
        Some("all"),
        "另一位 Human 没写过 ⇒ 它自己那一行仍是 all（个人事实不成对）"
    );
}

/// 只有群房有这一档：`direct` / `human_direct` 的**成员**也是 404；
/// 非成员 / 未知 / 坏 id 同一个 404（与非成员同形，不泄露房间是否存在）。
#[tokio::test]
async fn two_person_rooms_and_non_members_are_all_room_not_found() {
    let f = fixture().await;

    // 我是成员，但这不是群房 ⇒ 404（两人房里没有「群消息」这一档）。
    for room in [&f.direct_room, &f.human_direct_room] {
        let (status, json) = put_attention(
            &f.router,
            Some(&f.human_token),
            room,
            Some(&serde_json::json!({"level": "mentions"})),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "two-person room: {json}");
        assert_eq!(json["error"]["code"].as_str(), Some("room_not_found"));
    }

    // 群房但我不是成员 ⇒ 同一个 404，两条认证面同形（各自用自己那一面的凭据：
    // Bob 不在那个群；Agent A 也不在）。
    for (status, json) in [
        put_attention(
            &f.router,
            Some(&f.second_human_token),
            &f.solo_group_room,
            Some(&serde_json::json!({"level": "mentions"})),
            &[],
        )
        .await,
        put_backend_attention(
            &f.router,
            Some(&f.agent_token),
            &f.solo_group_room,
            Some(&serde_json::json!({"level": "mentions"})),
        )
        .await,
    ] {
        assert_eq!(status, StatusCode::NOT_FOUND, "non-member: {json}");
        assert_eq!(json["error"]["code"].as_str(), Some("room_not_found"));
    }

    // 未知房间与坏 id：都是同一个 404（不泄露存在性，也不区分格式错）。
    for room in [Uuid::new_v4().to_string(), "not-a-uuid".to_string()] {
        let (status, json) = put_attention(
            &f.router,
            Some(&f.human_token),
            &room,
            Some(&serde_json::json!({"level": "mentions"})),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "unknown/bad id: {json}");
        assert_eq!(json["error"]["code"].as_str(), Some("room_not_found"));
    }

    // 对照：**同一间** solo 群，它是 owner 与 active 成员 ⇒ 写得进去。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.solo_group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "member of the group: {json}");

    // 所有失败都没留下别人的行：Agent A 的目录里那一间仍是 all。
    let (status, json) = send(
        &f.router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&format!("Bearer {}", f.agent_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(
        json["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["room_id"].as_str() != Some(f.solo_group_room.as_str())),
        "A 不是那个群的成员 ⇒ 目录里本来就没有它: {json}"
    );
}

/// 两条认证面**同形**：Agent 走 Backend 路由写的是**它自己**那一行，
/// 与 Human 在同一个房间里的档互不覆盖；目录行也带同一格。
#[tokio::test]
async fn backend_route_is_the_same_shape_and_writes_only_that_agents_row() {
    let f = fixture().await;

    // Agent 写自己那一行。
    let (status, json) = put_backend_attention(
        &f.router,
        Some(&f.agent_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agent set mentions: {json}");
    assert_eq!(json["attention"].as_str(), Some("mentions"));
    assert!(json.get("idempotency_replayed").is_none(), "{json}");

    // Human 在同一间房写 all（显式写，不是"缺行"）。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "all"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "human set all: {json}");

    // 两人各是各的：两份列表各自读到自己的档。
    assert_eq!(
        backend_attention(&f.router, &f.agent_token, &f.group_room)
            .await
            .as_str(),
        Some("mentions"),
        "Agent 那一行是它自己写的"
    );
    assert_eq!(
        human_attention(&f.router, &f.human_token, &f.group_room)
            .await
            .as_str(),
        Some("all"),
        "Human 那一行是它自己写的（不被 Agent 覆盖）"
    );

    // 反方向再验一次：Agent 写 all，Human 仍 mentions。
    let (status, _) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = put_backend_attention(
        &f.router,
        Some(&f.agent_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "all"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        backend_attention(&f.router, &f.agent_token, &f.group_room)
            .await
            .as_str(),
        Some("all")
    );
    assert_eq!(
        human_attention(&f.router, &f.human_token, &f.group_room)
            .await
            .as_str(),
        Some("mentions")
    );

    // 两人房在 Agent 目录里同样是 all（没有这一档 ⇒ 恒默认），而且这一格
    // **每一行都在**（不是只在群房出现）。
    assert_eq!(
        backend_attention(&f.router, &f.agent_token, &f.direct_room)
            .await
            .as_str(),
        Some("all")
    );
    let (status, json) = send(
        &f.router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&format!("Bearer {}", f.agent_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    for row in json["rooms"].as_array().unwrap() {
        assert!(
            row.get("attention").is_some(),
            "目录每一行都必须带 attention: {row}"
        );
    }
}

/// body 形状与认证边界：只认 `{level}` 与两个词；超限 413；失败不改档；
/// 两条路由各自的认证面一字不变（Human token 过不了 Backend 路由）。
#[tokio::test]
async fn body_contract_is_enforced_and_auth_boundaries_are_unchanged() {
    let f = fixture().await;

    // 未知字段（含"写谁的"）：deny_unknown_fields ⇒ 400。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({
            "level": "mentions",
            "participant_id": "someone-else",
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown field: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_request"));

    // 闭集之外的取值：400（不是 404、也不是 500）。
    for level in ["none", "ALL", "", "all,mentions"] {
        let (status, json) = put_attention(
            &f.router,
            Some(&f.human_token),
            &f.group_room,
            Some(&serde_json::json!({"level": level})),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "level {level:?}: {json}");
        assert_eq!(json["error"]["code"].as_str(), Some("invalid_request"));
    }

    // 空 body（带 content-type）⇒ 400。
    let (status, json) = put_attention_at(
        &f.router,
        Some(&f.human_token),
        "/v1/rooms/",
        &f.group_room,
        None,
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty body: {json}");

    // 超过 route-local 1 KiB ⇒ 413 payload_too_large。
    let big = serde_json::json!({"level": "mentions", "pad": "x".repeat(2048)});
    let (status, json) = put_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
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

    // 五次失败之后档仍在起点（失败不写）。
    assert_eq!(
        human_attention(&f.router, &f.human_token, &f.group_room)
            .await
            .as_str(),
        Some("all")
    );

    // 认证面：缺凭据 401 authentication_required。两条路由各自的词表一字不变——
    // Human 侧认不出的 bearer 是 `invalid_session`（既有语义），Backend 侧认不出
    // 的是 `invalid_credential`；**Human token 过不了 Backend 路由**，反之亦然。
    let (status, json) = put_attention(
        &f.router,
        None,
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no auth: {json}");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("authentication_required")
    );

    let (status, json) = put_attention(
        &f.router,
        Some("clr_bc_not-a-real-token"),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "bad token: {json}");
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_session"));

    let (status, json) = put_backend_attention(
        &f.router,
        Some(&f.human_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "human token on backend route: {json}"
    );
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_credential"));

    // Backend token 也过不了 Human 侧路由（两条认证面互不冒充）。
    let (status, json) = put_attention(
        &f.router,
        Some(&f.agent_token),
        &f.group_room,
        Some(&serde_json::json!({"level": "mentions"})),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "backend token on human route: {json}"
    );
    assert_eq!(json["error"]["code"].as_str(), Some("invalid_session"));

    // 四次 401 也没写进任何东西。
    assert_eq!(
        human_attention(&f.router, &f.human_token, &f.group_room)
            .await
            .as_str(),
        Some("all")
    );
}
