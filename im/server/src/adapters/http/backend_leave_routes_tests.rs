//! Agent 退房端点 HTTP 契约测试（CORE_V1_AGENT_LEAVE.md，切片 M2）：
//! BackendAuth 认证（401 语义对齐既有 Backend 端点、Human token 一律拒绝）、
//! 「非 owner 的成员周期可以由它自己以 closed_by='leave' 关闭」这一条规则在
//! 群与 direct 房上的表现、human_direct 的出口不在这里（404）、以及 **Human 侧
//! 端点行为不变**（group member 204 / group owner 409）。断言全走公共 HTTP
//! 协议，不直读 SQLite。
//!
//! "member.left 事实与投递" 不在这里判：它的权威证据是存储层用例
//! `human_event_store_tests::agent_leave_produces_the_same_member_left_fact_as_a_human_leave`
//! （fact / delivery 两层逐条数），本文件只判端点契约。

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

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    human_token: String,
    agent_token: String,
    /// 第二位 Human（Bob）的 id 与 token：Human 侧回归用例要用（member 退群）。
    bob_id: String,
    bob_token: String,
    /// Alice(owner) + Agent A(member) 的群。
    group_with_agent: String,
    /// 只有 Alice 的群（A 不是成员）。
    group_without_agent: String,
    /// Alice(owner) + Agent A(member) 的 Direct Room。
    direct_room: String,
}

/// 通过 schema 直接创建第二个 Human 并补发 Session（既有测试同款 fixture
/// 手法；断言面仍全走公共协议）。
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
    let a = identity::bootstrap_human(&identity, "Alice")
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
    let human_token = a.token.as_str().to_owned();

    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/agents",
        &human_token,
        Some("lv-a-1"),
        Some(&serde_json::json!({"display_name": "Agent A"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create agent A: {json}");
    let agent_a = json["agent"]["id"].as_str().unwrap().to_owned();
    let agent_token = credential_for(&router, &human_token, &agent_a, "lv-b").await;

    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/direct-rooms",
        &human_token,
        Some("lv-d-1"),
        Some(&serde_json::json!({"agent_id": agent_a.clone()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create direct room: {json}");
    let direct_room = json["room"]["id"].as_str().unwrap().to_owned();

    let mut groups = Vec::new();
    for (key, name, with_agent) in [
        ("lv-g-1", "有 Agent 的群", true),
        ("lv-g-2", "只有人的群", false),
    ] {
        let (status, json, _) = request_json(
            &router,
            Method::POST,
            "/v1/groups",
            &human_token,
            Some(key),
            Some(&serde_json::json!({"name": name})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create group {name}: {json}");
        let group = json["room"]["id"].as_str().unwrap().to_owned();
        if with_agent {
            let (status, json, _) = request_json(
                &router,
                Method::POST,
                &format!("/v1/rooms/{group}/members"),
                &human_token,
                Some(&format!("{key}-i")),
                Some(&serde_json::json!({"participant_id": agent_a.clone()})),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "invite A into {name}: {json}");
        }
        groups.push(group);
    }

    Fixture {
        _dir: dir,
        router,
        human_token,
        agent_token,
        bob_id,
        bob_token,
        group_with_agent: groups[0].clone(),
        group_without_agent: groups[1].clone(),
        direct_room,
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
        Some(&serde_json::json!({"label": "leave e2e"})),
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
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if json_body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    if let Some(key) = key {
        headers.push((KEY_HEADER, key));
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

const KEY_HEADER: &str = "idempotency-key";

/// 一次退房（Bearer 可空 = 无 Authorization 头；无体、无幂等键）。
async fn leave(
    router: &axum::Router,
    token: Option<&str>,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(Method::POST).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let _ = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    // 空体解析成 Null（**不是** `{}`）：204 与"回了一个空 JSON 对象"要能分开。
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn backend_leave(
    router: &axum::Router,
    token: &str,
    room: &str,
) -> (StatusCode, serde_json::Value) {
    leave(
        router,
        Some(token),
        &format!("/v1/backend/rooms/{room}/leave"),
    )
    .await
}

async fn backend_rooms(router: &axum::Router, token: &str) -> serde_json::Value {
    let (status, json, _) =
        request_json(router, Method::GET, "/v1/backend/rooms", token, None, None).await;
    assert_eq!(status, StatusCode::OK, "directory: {json}");
    json
}

fn lists_room(directory: &serde_json::Value, room: &str) -> bool {
    directory["rooms"]
        .as_array()
        .expect("rooms array")
        .iter()
        .any(|r| r["room_id"].as_str() == Some(room))
}

/// 群里退房：204 无体；此后该房对这位 Agent 彻底消失（通讯录不再列、历史 404）；
/// 第二次退房 404（周期已关闭，不再是成员）——与 Human 侧同。
#[tokio::test]
async fn agent_leaves_a_group_and_the_room_disappears_for_it() {
    let f = fixture().await;
    assert!(
        lists_room(
            &backend_rooms(&f.router, &f.agent_token).await,
            &f.group_with_agent
        ),
        "退房前：群里能看到这间房"
    );

    let (status, json) = backend_leave(&f.router, &f.agent_token, &f.group_with_agent).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "退房: {json}");
    assert!(json.is_null(), "204 无体: {json}");

    let directory = backend_rooms(&f.router, &f.agent_token).await;
    assert!(
        !lists_room(&directory, &f.group_with_agent),
        "退房后：通讯录不再列这间房: {directory}"
    );
    // 退房后读历史：**403 not_room_member**——这是既有 Backend 消息/历史端点的
    // 口径（与 Human 侧、与读游标/退房端点的 404 不一致，审计编号 **C2**，待裁）。
    // 本切片不改既有端点语义，因此这里如实钉住今天的答案，不假装它已经是 404。
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/backend/rooms/{}/messages", f.group_with_agent),
        &f.agent_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "退房后历史: {json}");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("not_room_member"),
        "{json}"
    );

    // 重复退房：404（不是 204）——周期已关闭，不再是成员。
    let (status, json) = backend_leave(&f.router, &f.agent_token, &f.group_with_agent).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "重复退房: {json}");
}

/// Agent 退出 Direct Room：它自己出局，**Human 留在房里**（房与历史都留着，
/// Human 照旧读得到）——「退出」不是「删房」，也不是「结束关系」。
#[tokio::test]
async fn agent_leaves_its_direct_room_while_the_human_keeps_it() {
    let f = fixture().await;
    let (status, json) = backend_leave(&f.router, &f.agent_token, &f.direct_room).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "Agent 退出 direct 房: {json}"
    );

    assert!(
        !lists_room(
            &backend_rooms(&f.router, &f.agent_token).await,
            &f.direct_room
        ),
        "Agent 侧：这间房不再出现在它的通讯录里"
    );

    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{}/members", f.direct_room),
        &f.human_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Human 侧照旧读得到成员名单: {json}");
    let members = json["members"].as_array().expect("members");
    let agent_member = members
        .iter()
        .find(|m| m["participant"]["kind"].as_str() == Some("agent"))
        .expect("成员名单里那位 Agent 还在（周期 closed，不删行）");
    assert_eq!(
        agent_member["membership"]["closed_by"].as_str(),
        Some("leave"),
        "成员投影如实说它离开了: {agent_member}"
    );
    assert!(
        agent_member["membership"]["left_at"].is_string(),
        "left_at 落定: {agent_member}"
    );
}

/// 非成员 / 未知房 / 形状非法：一律 404（不泄露存在性）。
#[tokio::test]
async fn non_member_unknown_and_malformed_targets_are_all_404() {
    let f = fixture().await;
    for path in [
        format!("/v1/backend/rooms/{}/leave", f.group_without_agent),
        format!("/v1/backend/rooms/{}/leave", Uuid::new_v4()),
        "/v1/backend/rooms/not-a-uuid/leave".to_string(),
    ] {
        let (status, json) = leave(&f.router, Some(&f.agent_token), &path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {json}");
    }
}

/// 认证：BackendAuth 专属——缺 header / 未知 token / **Human token** 一律 401。
#[tokio::test]
async fn auth_is_backend_only_and_human_tokens_are_rejected() {
    let f = fixture().await;
    let path = format!("/v1/backend/rooms/{}/leave", f.group_with_agent);

    let (status, json) = leave(&f.router, None, &path).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "缺 header: {json}");

    for token in [
        "celira-backend-unknown-token".to_string(),
        f.human_token.clone(),
    ] {
        let (status, json) = leave(&f.router, Some(&token), &path).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token}: {json}");
    }

    // 负对照：那个 Human token 在 Human 侧自己的退房端点上是有效的（404 而不是 401
    // ——它确实不是那个群的成员）。
    let (status, _) = leave(
        &f.router,
        Some(&f.human_token),
        &format!("/v1/rooms/{}/leave", f.group_without_agent),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "Human 侧：owner 退群 409 owner_transfer_required"
    );
}

/// **Human 侧端点行为不变**：group owner 退群照旧 409；member 退群照旧 204。
#[tokio::test]
async fn human_leave_endpoint_is_unchanged() {
    let f = fixture().await;

    // owner（Alice）退自己建的群 → 409 先转让。
    let (status, json) = leave(
        &f.router,
        Some(&f.human_token),
        &format!("/v1/rooms/{}/leave", f.group_without_agent),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "owner 退群: {json}");
    assert_eq!(
        json["error"]["code"].as_str(),
        Some("owner_transfer_required"),
        "{json}"
    );

    // member 退群 → 204：新群 + 第二位 Human（先建立 accepted 关系才能拉进群）。
    let (status, json, _) =
        request_json(&f.router, Method::GET, "/v1/me", &f.human_token, None, None).await;
    assert_eq!(status, StatusCode::OK, "me: {json}");
    let alice_id = json["participant"]["id"]
        .as_str()
        .or_else(|| json["id"].as_str())
        .expect("me 回执里要有自己的 participant id")
        .to_owned();

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        &f.human_token,
        Some("lv-c-1"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "contact request: {json}");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{alice_id}/accept"),
        &f.bob_token,
        Some("lv-c-2"),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "contact accept: {json}");

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/groups",
        &f.human_token,
        Some("lv-h-1"),
        Some(&serde_json::json!({"name": "member 退群用例"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create group: {json}");
    let group = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group}/members"),
        &f.human_token,
        Some("lv-h-2"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite bob: {json}");

    let (status, json) = leave(
        &f.router,
        Some(&f.bob_token),
        &format!("/v1/rooms/{group}/leave"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "member 退群: {json}");
}
