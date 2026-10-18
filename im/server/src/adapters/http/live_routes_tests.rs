//! 主体进行态 HTTP handler 测试（契约 `CORE_V1_AGENT_PROTOCOL.md` §6）。
//!
//! 真实 SQLite（成员裁决）+ 真实进程内进行态（`VolatileLive` + 两个连接
//! 事实来源）的组合测试，覆盖这一片的全部判据：
//! - 读（两条端点**同一投影**）：成员名单含**人与 Agent**（修正旧 Live
//!   State 只列 Agent）、`kind` 如实、未知/非成员统一 404 `room_not_found`；
//! - 写（人机各一条 typing）：204、按房隔离、按 TTL 过期、发消息即清除、
//!   非成员 404、带 body 400；
//! - **零执行面**：响应里没有 phase / stream_id / revision / body /
//!   instance_id（旧字段不许以任何名字复活）；
//! - 在线由**连接**给出：没有任何连接时全员 `online: false`；Human 挂了
//!   WS 或 Agent 挂了门铃流才为 true（门铃那一段在 `tests/blackbox.rs`
//!   用真实进程 + 真 SSE 连接证明）。
//!
//! ⚠ 2026-09-28：本文件整体重写——旧的心跳/Activity/独立 Live WS 用例随
//! 执行面一起删除（主人裁决「不需要看到执行过程」）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::identity;

const KEY_HEADER: &str = "idempotency-key";
const AUTH_JSON: &str = "application/json";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    /// 同一接线束的句柄：测试可以直接操纵"连接"与"进行态"来验判据，
    /// 而不必绕过协议直读内部表。
    wiring: crate::adapters::live::LiveWiring,
    human_token: String,
    human_id: String,
    agent_id: String,
    backend_token: String,
    room_id: String,
    pool: sqlx::SqlitePool,
}

/// 一个 Human（Alice）+ 一支 Agent（小满）+ 一间 Direct Room：
/// 三人（俩主体）同房，正好验"人与 Agent 同形"。
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let human = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap alice");
    let human_token = human.token.as_str().to_owned();
    let human_id = human.participant.id.as_str().to_owned();

    let wiring = crate::adapters::live::LiveWiring::new_volatile();
    let router = build_router(
        Arc::new(store),
        Arc::new(identity),
        Arc::new(SqliteAgentStore::new(pool.clone())),
        Arc::new(SqliteRoomStore::new(pool.clone())),
        Arc::new(SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(SqliteEventStore::new(pool.clone())),
        Arc::new(SqliteContactStore::new(pool.clone())),
        Arc::new(
            crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(pool.clone()),
        ),
        wiring.clone(),
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

    let agent_id = create_agent(&router, &human_token, "小满", "a-1").await;
    let room_id = create_room(&router, &human_token, &agent_id, "r-1").await;
    let (_credential_id, backend_token) =
        issue_backend_token(&router, &human_token, &agent_id).await;

    Fixture {
        _dir: dir,
        router,
        wiring,
        human_token,
        human_id,
        agent_id,
        backend_token,
        room_id,
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
    assert_eq!(status, StatusCode::CREATED, "agent must be created: {json}");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

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
    assert_eq!(status, StatusCode::CREATED, "room must be created: {json}");
    json["room"]["id"].as_str().unwrap().to_owned()
}

async fn issue_backend_token(
    router: &axum::Router,
    human_token: &str,
    agent_id: &str,
) -> (String, String) {
    let body = serde_json::json!({ "label": "Native Runtime" }).to_string();
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

/// 建一个群房并把 Agent 邀进来（返回 group room id）。
async fn create_group_with_agent(
    f: &Fixture,
    name: &str,
    group_key: &str,
    invite_key: &str,
) -> String {
    let body = serde_json::json!({ "name": name }).to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.human_token)),
        &[(KEY_HEADER, group_key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "group create: {json}");
    let room_id = json["room"]["id"].as_str().unwrap().to_owned();

    let body = serde_json::json!({ "participant_id": f.agent_id }).to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/members"),
        Some(&format!("Bearer {}", f.human_token)),
        &[(KEY_HEADER, invite_key), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite agent: {json}");
    room_id
}

/// 造第二个 Human（bootstrap 只允许一次，所以直接建 participant + session）。
async fn add_human(pool: &sqlx::SqlitePool, identity: &SqliteIdentityStore, name: &str) -> String {
    use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
    use crate::domain::participant::ParticipantId;
    use crate::domain::time::UtcMillis;
    use crate::domain::token::OpaqueToken;
    use crate::ports::identity::{IdentityStore as _, NewSession};

    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', ?, 0, NULL)",
    )
    .bind(&id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert second human");
    let token = OpaqueToken::generate().expect("token");
    let session = HumanSession::new(
        SessionId(uuid::Uuid::new_v4().to_string()),
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
    token.as_str().to_owned()
}

/// 取快照里某位成员那一行。
fn member<'a>(snapshot: &'a serde_json::Value, participant_id: &str) -> &'a serde_json::Value {
    snapshot["members"]
        .as_array()
        .expect("members array")
        .iter()
        .find(|row| row["participant_id"] == participant_id)
        .expect("member present in snapshot")
}

/// 读人侧快照（Human Session）。
async fn snapshot_human(f: &Fixture) -> serde_json::Value {
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{}/live", f.room_id),
        Some(&format!("Bearer {}", f.human_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "snapshot: {json}");
    json
}

/// 读 Agent 侧快照（BackendAuth）。
async fn snapshot_backend(f: &Fixture) -> serde_json::Value {
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/backend/rooms/{}/live", f.room_id),
        Some(&format!("Bearer {}", f.backend_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "backend snapshot: {json}");
    json
}

// ─────────────────────────── 读面 ───────────────────────────

#[tokio::test]
async fn snapshot_lists_every_member_human_and_agent_alike() {
    let f = fixture().await;
    let snap = snapshot_human(&f).await;

    assert_eq!(snap["room_id"], f.room_id);
    let members = snap["members"].as_array().unwrap();
    assert_eq!(
        members.len(),
        2,
        "人机同形：Direct Room 里两位主体都在名单上（旧 Live State 只列 Agent）"
    );

    let human_row = member(&snap, &f.human_id);
    assert_eq!(human_row["kind"], "human");
    assert_eq!(human_row["display_name"], "Alice");
    let agent_row = member(&snap, &f.agent_id);
    assert_eq!(agent_row["kind"], "agent");
    assert_eq!(agent_row["display_name"], "小满");

    // 什么都没连：全员离线、无人正在写。
    for row in members {
        assert_eq!(row["online"], false, "没有连接就没有在线：{row}");
        assert_eq!(row["typing"], false, "没人声明过在写：{row}");
    }
}

#[tokio::test]
async fn snapshot_has_zero_execution_surface_fields() {
    let f = fixture().await;
    let snap = snapshot_human(&f).await;
    let row = member(&snap, &f.agent_id);

    // 旧 Live State 的执行面字段一律不许复活（换名字也不行）。
    for forbidden in [
        "activity",
        "phase",
        "stream_id",
        "revision",
        "body",
        "presence",
    ] {
        assert!(
            row.get(forbidden).is_none(),
            "执行面字段 `{forbidden}` 不得出现在进行态投影里：{row}"
        );
    }
    assert!(
        snap.get("agents").is_none(),
        "投影的名字是 members（人与 Agent 同形），不是 agents"
    );
}

#[tokio::test]
async fn both_auth_faces_return_the_same_projection() {
    let f = fixture().await;
    // 先让 Agent 走一次 BackendAuth（拉通讯录）：这会把它标成"被看见"，
    // 于是两侧读到的在线状态才是可比的（否则"刚认证过"这一事实只体现在
    // 后读的那一侧——那正是连接派生的应有表现，见下一条断言）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&format!("Bearer {}", f.backend_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let human_view = snapshot_human(&f).await;
    let backend_view = snapshot_backend(&f).await;

    // request_id 天然不同，其余逐字段相同。
    let mut a = human_view.clone();
    let mut b = backend_view.clone();
    a["request_id"] = serde_json::json!("x");
    b["request_id"] = serde_json::json!("x");
    assert_eq!(a, b, "两条端点同一段投影，只在适配器换认证面");
    assert_eq!(
        member(&a, &f.agent_id)["online"],
        true,
        "Agent 刚认证过 ⇒ 在线（拉取式 Backend 的兜底判据）"
    );
}

#[tokio::test]
async fn snapshot_rejects_outsider_and_unknown_room_uniformly() {
    let f = fixture().await;
    // 第二个 Human：不是这间房的人。
    let outsider_token = add_human(&f.pool, &SqliteIdentityStore::new(f.pool.clone()), "Bob").await;

    for path in [
        format!("/v1/rooms/{}/live", f.room_id),
        "/v1/rooms/00000000-0000-4000-8000-000000000000/live".to_owned(),
        "/v1/rooms/not-a-uuid/live".to_owned(),
    ] {
        let (status, json, _) = send(
            &f.router,
            Method::GET,
            &path,
            Some(&format!("Bearer {outsider_token}")),
            &[],
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "非成员/未知房统一 404: {json}"
        );
        assert_eq!(json["error"]["code"], "room_not_found");
    }
}

// ─────────────────────────── 写面：typing ───────────────────────────

#[tokio::test]
async fn typing_is_visible_to_peers_then_expires_by_its_own_ttl() {
    let f = fixture().await;

    let (status, _, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/typing", f.room_id),
        Some(&format!("Bearer {}", f.human_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "typing 是 204：声明不是事实"
    );

    let snap = snapshot_human(&f).await;
    assert_eq!(
        member(&snap, &f.human_id)["typing"],
        true,
        "自己声明了在写，同房成员就该看得见"
    );
    assert_eq!(
        member(&snap, &f.agent_id)["typing"],
        false,
        "别人没写就不许亮"
    );

    // TTL 过期：把进行态表的时间基准往后推（等价于"6 秒没人续报"）。
    let expired_at = crate::domain::time::UtcMillis::from_millis(
        crate::domain::time::UtcMillis::now().millis()
            + crate::domain::live::TYPING_TTL.as_millis() as i64,
    );
    let live = f.wiring.presence();
    let room = crate::domain::room::RoomId::parse_canonical(&f.room_id).unwrap();
    assert!(
        live.typing_in_room(&room, expired_at).is_empty(),
        "超过 TYPING_TTL 即视为没在写（没有任何人来清理，它自己就没了）"
    );
}

#[tokio::test]
async fn agent_typing_uses_the_same_rule_as_human() {
    let f = fixture().await;

    let (status, _, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{}/typing", f.room_id),
        Some(&format!("Bearer {}", f.backend_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Human 侧立刻看得见 Agent 正在写——人机同形，一条判据。
    let snap = snapshot_human(&f).await;
    assert_eq!(member(&snap, &f.agent_id)["typing"], true);
    assert_eq!(member(&snap, &f.human_id)["typing"], false);
}

#[tokio::test]
async fn typing_is_scoped_to_the_declaring_room() {
    let f = fixture().await;
    // 另开一间房：同一对主体的 direct 房至多一间（「一间房 = 一段关系」），
    // 所以第二间用**群**，把 Agent 邀进来。
    let other_room = create_group_with_agent(&f, "项目群", "g-1", "gm-1").await;

    let (status, _, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/typing", f.room_id),
        Some(&format!("Bearer {}", f.human_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{other_room}/live"),
        Some(&format!("Bearer {}", f.human_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        member(&json, &f.human_id)["typing"],
        false,
        "进行态按房隔离：A 房的写作不在 B 房亮"
    );
}

#[tokio::test]
async fn sending_a_message_supersedes_own_typing() {
    let f = fixture().await;

    let (status, _, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/typing", f.room_id),
        Some(&format!("Bearer {}", f.human_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 发出消息：进行态被"我说完了"取代。
    let body = serde_json::json!({ "body": "在的" }).to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/messages", f.room_id),
        Some(&format!("Bearer {}", f.human_token)),
        &[(KEY_HEADER, "m-1"), ("content-type", AUTH_JSON)],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    let snap = snapshot_human(&f).await;
    assert_eq!(
        member(&snap, &f.human_id)["typing"],
        false,
        "消息发出后自己的「正在写」立刻消失"
    );
}

#[tokio::test]
async fn typing_rejects_outsider_unknown_room_and_any_body() {
    let f = fixture().await;
    let outsider_token = add_human(&f.pool, &SqliteIdentityStore::new(f.pool.clone()), "Bob").await;

    // 非成员 / 未知房：统一 404（不泄露房间是否存在）。
    for path in [
        format!("/v1/rooms/{}/typing", f.room_id),
        "/v1/rooms/00000000-0000-4000-8000-000000000000/typing".to_owned(),
    ] {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            &path,
            Some(&format!("Bearer {outsider_token}")),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
        assert_eq!(json["error"]["code"], "room_not_found");
    }

    // typing 没有参数：带 body 一律 400（不静默忽略）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/typing", f.room_id),
        Some(&format!("Bearer {}", f.human_token)),
        &[("content-type", AUTH_JSON)],
        Some(
            serde_json::json!({ "phase": "responding" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // 请求体被拒时不得留下进行态副作用。
    let snap = snapshot_human(&f).await;
    assert_eq!(member(&snap, &f.human_id)["typing"], false);
}

#[tokio::test]
async fn typing_requires_authentication() {
    let f = fixture().await;
    for (path, auth) in [
        (format!("/v1/rooms/{}/typing", f.room_id), None),
        (format!("/v1/backend/rooms/{}/typing", f.room_id), None),
    ] {
        let (status, json, _) = send(&f.router, Method::POST, &path, auth, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
        assert_eq!(json["error"]["code"], "authentication_required");
    }
}

// ─────────────────────────── 在线 = 连接在 ───────────────────────────

#[tokio::test]
async fn online_follows_the_connection_not_a_heartbeat() {
    let f = fixture().await;
    let live = f.wiring.presence();
    let human = crate::domain::participant::ParticipantId(f.human_id.clone());
    let agent = crate::domain::participant::ParticipantId(f.agent_id.clone());

    // ① 没有任何连接：离线。
    let snap = snapshot_human(&f).await;
    assert_eq!(member(&snap, &f.human_id)["online"], false);
    assert_eq!(member(&snap, &f.agent_id)["online"], false);

    // ② Human 挂上 WS（订阅表里有活引用）⇒ 立刻在线——没有心跳、没有上报。
    let (wake_tx, _wake_rx) = tokio::sync::mpsc::channel::<()>(1);
    let wake_conn = Arc::new(wake_tx);
    crate::ports::human_event::HumanEventNotifier::subscribe(
        f.wiring.human_notifier().as_ref(),
        &human,
        wake_conn.clone(),
    );
    let snap = snapshot_human(&f).await;
    assert_eq!(
        member(&snap, &f.human_id)["online"],
        true,
        "有活着的 WS 连接即在线"
    );
    assert_eq!(
        member(&snap, &f.agent_id)["online"],
        false,
        "另一位的在线状态不跟着变"
    );

    // ③ 连接断开（丢弃强引用）⇒ 在线自己消失，不需要任何人上报离线。
    drop(wake_conn);
    let snap = snapshot_human(&f).await;
    assert_eq!(
        member(&snap, &f.human_id)["online"],
        false,
        "连接没了，在线就没了"
    );

    // ④ 拉取式 Backend（不挂门铃）：`mark_seen` 的宽限窗口内算在线，
    //    超过 `SEEN_TTL` 就不算——这是"最近被看见"的固有代价。
    let now = crate::domain::time::UtcMillis::now();
    live.mark_seen(&agent, now);
    assert!(
        live.seen_recently(&agent, now),
        "被看见过 ⇒ 在线（认证路径的副产品，不是心跳上报）"
    );
    let beyond = crate::domain::time::UtcMillis::from_millis(
        now.millis() + crate::domain::live::SEEN_TTL.as_millis() as i64,
    );
    assert!(!live.seen_recently(&agent, beyond), "超过窗口即离线");
}

#[tokio::test]
async fn agent_online_comes_from_the_doorbell_stream_gate() {
    let f = fixture().await;
    let agent = crate::domain::participant::ParticipantId(f.agent_id.clone());
    let gate = f.wiring.streams();

    assert!(!gate.is_connected(&agent), "没挂门铃就是离线");
    let permit = gate.acquire(&agent).expect("first stream is allowed");
    assert!(gate.is_connected(&agent), "挂着门铃流即在线");

    let snap = snapshot_backend(&f).await;
    assert_eq!(
        member(&snap, &f.agent_id)["online"],
        true,
        "Agent 读自己那间房时也如实看到自己在线的判据与别人一致"
    );

    drop(permit);
    assert!(!gate.is_connected(&agent), "流一断，在线立刻消失");
}

#[tokio::test]
async fn backend_auth_marks_the_agent_seen_so_polling_backends_are_online() {
    let f = fixture().await;
    let agent = crate::domain::participant::ParticipantId(f.agent_id.clone());
    let live = f.wiring.presence();
    let now = crate::domain::time::UtcMillis::now();

    assert!(!live.seen_recently(&agent, now), "还没认证过：不算被看见");

    // 任何一次真实的 BackendAuth 调用（这里用通讯录）都算"被看见"。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/rooms",
        Some(&format!("Bearer {}", f.backend_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    assert!(
        live.seen_recently(&agent, crate::domain::time::UtcMillis::now()),
        "认证成功即被看见——拉取式 Backend 靠这条判在线"
    );
}
