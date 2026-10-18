//! Agent 读取面端点 HTTP 契约测试（CORE_V1_AGENT_READ_SURFACE.md）：6 条纯读
//! 端点的认证互斥（Human token 一律 401）、房间可见性（非成员 fail closed）、
//! 与 Human 侧**同一投影**（逐字段比对）、目标成员约束、头像缺失、自我读取
//! 形状、revoked fail closed、以及**读取无副作用**（前后行数不变）。
//! （原 7 条：`GET /v1/backend/publication` 随 Agent 服务层退役删除，2026-09-28。）
//!
//! 断言全走公共 HTTP 协议，不直读 SQLite 冒充成功证据；SQLite 只用于
//! 「没有越界对象 / 行数不变」的反向证明。

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

/// 本切片的 6 条端点（认证互斥与 fail-closed 用例逐条打）。
///
/// ⚠ 2026-09-28：原为 7 条 —— 第 7 条 `GET /v1/backend/publication`（读自己的
/// Service Publication）随 **Agent 服务层整体退役**删除。
const READ_SURFACE_PATHS: [&str; 6] = [
    "/v1/backend/rooms/{room}/members",
    "/v1/backend/rooms/{room}/member-presentations",
    "/v1/backend/rooms/{room}/members/{member}/profile",
    "/v1/backend/rooms/{room}/members/{member}/presentation",
    "/v1/backend/rooms/{room}/members/{member}/avatar",
    "/v1/backend/profile",
];

struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
    router: axum::Router,
    human_token: String,
    agent_a: String,
    agent_b: String,
    agent_a_token: String,
    agent_b_token: String,
    direct_room: String,
    group_room: String,
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

    // Direct Room（Human + A）：B 不是成员 —— 非成员 fail closed 的证据。
    let (status, json, _) = request_json(
        &router,
        Method::POST,
        "/v1/direct-rooms",
        &human_token,
        Some("d-1"),
        Some(&serde_json::json!({"agent_id": agent_a.clone()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create direct room");
    let direct_room = json["room"]["id"].as_str().unwrap().to_owned();

    // Group Room（Human + A + B）：三方同房 —— 同房成员读取面的证据。
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
        pool,
        router,
        human_token,
        agent_a,
        agent_b,
        agent_a_token,
        agent_b_token,
        direct_room,
        group_room,
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
        Some(&serde_json::json!({"label": "read surface e2e"})),
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

/// 以 backend token 打一条读取面端点（GET，无 body）。
async fn backend_get(
    router: &axum::Router,
    token: &str,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, json, _) = send(
        router,
        Method::GET,
        path,
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    (status, json)
}

/// 把 6 条路径模板按 fixture 的 room/member 展开。
fn expand(fixture: &Fixture, template: &str) -> String {
    template
        .replace("{room}", &fixture.group_room)
        .replace("{member}", &fixture.agent_b)
}

/// 读取面的副作用反向证明：这些表的行数在 6 条读端点前后必须完全不变。
async fn row_counts(pool: &SqlitePool) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for table in [
        "room_memberships",
        "read_cursors",
        "read_cursor_advancements",
        "human_event_facts",
        "human_event_deliveries",
        "agent_events",
        "messages",
        "participant_profile_versions",
        "room_presentation_versions",
    ] {
        let (count,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap_or_else(|e| panic!("count {table}: {e}"));
        out.push((table.to_owned(), count));
    }
    out
}

#[tokio::test]
async fn member_list_projection_is_identical_to_human_side() {
    let f = fixture().await;
    let path = format!("/v1/backend/rooms/{}/members", f.group_room);
    let (status, agent_json) = backend_get(&f.router, &f.agent_a_token, &path).await;
    assert_eq!(status, StatusCode::OK, "agent reads members: {agent_json}");

    let (status, human_json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{}/members", f.group_room),
        &f.human_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "human reads members");

    // 逐字段同投影：唯一允许的差异是 request_id（每次请求唯一）。
    let mut agent_members = agent_json["members"].clone();
    let mut human_members = human_json["members"].clone();
    let sort = |v: &mut serde_json::Value| {
        v.as_array_mut().unwrap().sort_by_key(|m| {
            m["participant"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        });
    };
    sort(&mut agent_members);
    sort(&mut human_members);
    assert_eq!(
        agent_members, human_members,
        "Agent 侧成员投影必须与 Human 侧逐字段相同"
    );
    assert_eq!(
        agent_json["members"].as_array().unwrap().len(),
        3,
        "Human + Agent A + Agent B"
    );
}

#[tokio::test]
async fn human_session_token_is_rejected_on_every_read_surface_endpoint() {
    let f = fixture().await;
    for template in READ_SURFACE_PATHS {
        let path = expand(&f, template);
        let (status, json) = backend_get(&f.router, &f.human_token, &path).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "Human token must not pass BackendAuth: {path} {json}"
        );
        assert_eq!(
            json["error"]["code"], "invalid_credential",
            "Human token on {path} must be invalid_credential (no oracle)"
        );
    }
}

#[tokio::test]
async fn missing_authorization_is_authentication_required() {
    let f = fixture().await;
    for template in READ_SURFACE_PATHS {
        let path = expand(&f, template);
        let (status, json, _) = send(&f.router, Method::GET, &path, None, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "no auth on {path}");
        assert_eq!(
            json["error"]["code"], "authentication_required",
            "no auth on {path}"
        );
    }
}

#[tokio::test]
async fn non_member_agent_gets_room_not_found() {
    let f = fixture().await;
    // Agent B 不是 direct room 的成员 —— 5 条房间范围端点全部 fail closed。
    for template in &READ_SURFACE_PATHS[..5] {
        let path = template
            .replace("{room}", &f.direct_room)
            .replace("{member}", &f.agent_a);
        let (status, json) = backend_get(&f.router, &f.agent_b_token, &path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "non-member must fail closed: {path} {json}"
        );
        assert_eq!(json["error"]["code"], "room_not_found", "on {path}");
    }
}

#[tokio::test]
async fn unknown_and_bad_room_id_fail_closed() {
    let f = fixture().await;
    for room in [
        Uuid::new_v4().to_string(),
        "not-a-uuid".to_owned(),
        String::new(),
    ] {
        let path = format!("/v1/backend/rooms/{room}/members");
        let (status, json) = backend_get(&f.router, &f.agent_a_token, &path).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "unknown/bad room must fail closed: {room} -> {status} {json}"
        );
    }
}

#[tokio::test]
async fn target_outside_room_gets_member_not_found() {
    let f = fixture().await;
    // Agent A 在 direct room 内，target = Agent B（不同房）→ 404 member_not_found。
    for template in &READ_SURFACE_PATHS[2..5] {
        let path = template
            .replace("{room}", &f.direct_room)
            .replace("{member}", &f.agent_b);
        let (status, json) = backend_get(&f.router, &f.agent_a_token, &path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "target outside room: {path} {json}"
        );
        let code = json["error"]["code"].as_str().unwrap_or_default();
        assert!(
            code == "member_not_found" || code == "avatar_not_found",
            "expected member/avatar not found on {path}, got {code}"
        );
    }
}

#[tokio::test]
async fn agent_reads_same_room_member_profile_written_by_owner() {
    let f = fixture().await;
    // Owner 为 Agent B 写一个 Profile 版本（写路径仍只对 Owner —— 本刀不碰）。
    let (status, json, _) = request_json(
        &f.router,
        Method::PUT,
        &format!("/v1/agents/{}/profile", f.agent_b),
        &f.human_token,
        Some("p-b-1"),
        Some(&serde_json::json!({
            "base_version": 0,
            "short_description": "记录员",
            "public_description": "负责把讨论整理成条目。"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "owner writes profile: {json}");

    // Agent A 在同房读 Agent B 的 Profile —— 必须看到同一份公共呈现。
    let path = format!(
        "/v1/backend/rooms/{}/members/{}/profile",
        f.group_room, f.agent_b
    );
    let (status, json) = backend_get(&f.router, &f.agent_a_token, &path).await;
    assert_eq!(status, StatusCode::OK, "agent reads member profile: {json}");
    assert_eq!(json["participant"]["id"], f.agent_b.as_str());
    assert_eq!(json["participant"]["kind"], "agent");
    assert_eq!(json["profile"]["profile_version"], 1);
    assert_eq!(json["profile"]["short_description"], "记录员");
    assert_eq!(
        json["profile"]["published_by_kind"], "human",
        "呈现由 Owner 发布 —— 本刀不改写路径"
    );

    // Human 侧同一路径必须给出同一 profile 投影。
    let (status, human_json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{}/members/{}/profile", f.group_room, f.agent_b),
        &f.human_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["profile"], human_json["profile"],
        "两侧 profile 同投影"
    );
    assert_eq!(json["participant"], human_json["participant"]);
}

#[tokio::test]
async fn agent_reads_own_profile_without_path_id() {
    // ⚠ 2026-09-28：本用例原名带 `_and_publication` —— 那时这一组还包含
    // `GET /v1/backend/publication`（读自己的 Service Publication）。服务层
    // 退役后那条端点整体删除（见同文件 `READ_SURFACE_PATHS` 与
    // `no_write_no_side_effects_on_read_surface`），本用例只钉 profile。
    let f = fixture().await;
    // 无版本 → null（不是 404，也不是空对象）。
    let (status, json) = backend_get(&f.router, &f.agent_a_token, "/v1/backend/profile").await;
    assert_eq!(status, StatusCode::OK, "self profile: {json}");
    assert!(json["profile"].is_null(), "no version yet -> null");

    // Owner 为 Agent A 写入 Profile 后，Agent 能读到自己的。
    let (status, json, _) = request_json(
        &f.router,
        Method::PUT,
        &format!("/v1/agents/{}/profile", f.agent_a),
        &f.human_token,
        Some("p-a-1"),
        Some(&serde_json::json!({
            "base_version": 0,
            "short_description": "助理",
            "public_description": null
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "owner writes A profile: {json}"
    );

    let (status, json) = backend_get(&f.router, &f.agent_a_token, "/v1/backend/profile").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["profile"]["profile_version"], 1);
    assert_eq!(json["profile"]["short_description"], "助理");
    assert_eq!(json["profile"]["participant_id"], f.agent_a.as_str());

    // 响应形状：身份只来自认证 —— 不得出现 Owner / Binding / Credential 字段。
    for forbidden in ["ownership", "binding", "credential", "token", "human_id"] {
        assert!(
            json.get(forbidden).is_none(),
            "self profile must not expose {forbidden}: {json}"
        );
    }
}

#[tokio::test]
async fn avatar_absent_is_avatar_not_found_and_member_presentations_is_ok() {
    let f = fixture().await;
    let path = format!(
        "/v1/backend/rooms/{}/members/{}/avatar",
        f.group_room, f.agent_b
    );
    let (status, json) = backend_get(&f.router, &f.agent_a_token, &path).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no avatar: {json}");
    assert_eq!(json["error"]["code"], "avatar_not_found");

    let path = format!("/v1/backend/rooms/{}/member-presentations", f.group_room);
    let (status, json) = backend_get(&f.router, &f.agent_a_token, &path).await;
    assert_eq!(status, StatusCode::OK, "member presentations: {json}");
    assert_eq!(
        json["members"].as_array().unwrap().len(),
        3,
        "active 成员全部在内（Human 的 Presentation 字段为 null）"
    );
    let human_row = json["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["participant"]["kind"] == "human")
        .expect("human member row present");
    assert!(
        human_row["room_role_name"].is_null(),
        "Human 无 Presentation"
    );
}

#[tokio::test]
async fn revoked_agent_credential_fails_closed_on_read_surface() {
    let f = fixture().await;
    let (status, _, _) = request_json(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{}", f.agent_a),
        &f.human_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "revoke agent A");

    for template in READ_SURFACE_PATHS {
        let path = expand(&f, template);
        let (status, json) = backend_get(&f.router, &f.agent_a_token, &path).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "revoked credential must fail closed: {path} {json}"
        );
        assert_eq!(json["error"]["code"], "invalid_credential", "on {path}");
    }
}

#[tokio::test]
async fn read_surface_has_no_side_effects_on_public_facts() {
    let f = fixture().await;
    let before = row_counts(&f.pool).await;
    for template in READ_SURFACE_PATHS {
        let path = expand(&f, template);
        let (status, _) = backend_get(&f.router, &f.agent_a_token, &path).await;
        if template.ends_with("/avatar") {
            // 无头像 → 404 是正确业务裁决；本条只证明「读不产生事实」。
            assert_eq!(status, StatusCode::NOT_FOUND, "no avatar -> 404: {path}");
        } else {
            assert_eq!(status, StatusCode::OK, "read {path}");
        }
    }
    let after = row_counts(&f.pool).await;
    assert_eq!(
        before, after,
        "读取面不得新增任何公共事实（无 cursor / 无 Event / 无投递位置）"
    );
}
