//! Agent 侧关系面的 HTTP 契约测试（批次三·第二刀「关系对主体开放」，
//! `docs/contracts/core-v1/CORE_V1_CONTACT_REVERSIBILITY.md`）：
//!
//! - **关系是两个主体之间的事实**：Human–Agent、Agent–Agent 都能建立，Agent 也能
//!   主动发起、自己应答、单方面解除；
//! - **同意归被申请的那一方**：`accept` / `decline` 只认"我是不是 addressee"，
//!   第三方（哪怕是同一支 Agent 的 Owner）答不了别人给它的申请；
//! - **认证面**：五条端点全部 BackendAuth（缺凭据 401 `authentication_required`、
//!   Human token 一律 401 `invalid_credential`）；
//! - **邀请资格随关系走**（与第一刀合起来的反证）：Agent 有了 accepted 关系之后，
//!   **它就能邀请那位 Human 进自己的群**——第一刀登记的"Agent owner 邀请不到任何人"
//!   当场消失；反向同理，Human owner 能邀请**自己并不拥有**、但与自己有关系的 Agent；
//! - **Human 侧形状**：请求体字段 `participant_id`（本刀由 `human_id` 改名），
//!   旧字段名一律 400 `invalid_request`（未知字段），响应形状一字未变。
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
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::identity::{IdentityStore as _, NewSession};

const KEY_HEADER: &str = "idempotency-key";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    /// Alice（主人）与她的 Agent A。
    alice_token: String,
    alice_id: String,
    agent_a: String,
    agent_a_token: String,
    /// Bob（第二位 Human）与他的 Agent B——用来验"不拥有也能靠关系邀请"。
    bob_token: String,
    bob_id: String,
    agent_b: String,
    agent_b_token: String,
}

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
    let alice = identity::bootstrap_human(&identity, "Alice")
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
    let alice_token = alice.token.as_str().to_owned();
    let alice_id = alice.participant.id.as_str().to_owned();

    let (agent_a, agent_a_token) = agent_with_credential(&router, &alice_token, "bc-a", "甲").await;
    let (agent_b, agent_b_token) = agent_with_credential(&router, &bob_token, "bc-b", "乙").await;

    Fixture {
        _dir: dir,
        router,
        alice_token,
        alice_id,
        agent_a,
        agent_a_token,
        bob_token,
        bob_id,
        agent_b,
        agent_b_token,
    }
}

/// 建一支 Agent 并签发它的 Backend Credential（公开 API 全链）。
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
        owner_token,
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
        owner_token,
        Some(&format!("{key}-b")),
        Some(&serde_json::json!({"label": "contact e2e"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        owner_token,
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
    token: &str,
    key: Option<&str>,
    json_body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    request_json_opt(router, method, path, Some(token), key, json_body).await
}

/// Bearer 可空（判 401 的两种形态：**没有** Authorization 头 vs 头在场但无效）。
async fn request_json_opt(
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
        builder = builder.header(KEY_HEADER, key);
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

/// 从关系列表里取出与某个 peer 的那一条。
fn contact_with<'a>(json: &'a serde_json::Value, peer_id: &str) -> Option<&'a serde_json::Value> {
    json["contacts"]
        .as_array()?
        .iter()
        .find(|c| c["peer"]["id"] == serde_json::json!(peer_id))
}

// ── 1. 认证面 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn backend_contact_endpoints_require_backend_auth() {
    let f = fixture().await;
    for (label, token, method, path, key) in [
        (
            "list, no header",
            None,
            Method::GET,
            "/v1/backend/contacts".to_owned(),
            None,
        ),
        (
            "list, human token",
            Some(f.alice_token.as_str()),
            Method::GET,
            "/v1/backend/contacts".to_owned(),
            None,
        ),
        (
            "request, human token",
            Some(f.alice_token.as_str()),
            Method::POST,
            "/v1/backend/contacts/requests".to_owned(),
            Some("bc-auth-1"),
        ),
        (
            "accept, human token",
            Some(f.alice_token.as_str()),
            Method::POST,
            format!("/v1/backend/contacts/{}/accept", f.alice_id),
            None,
        ),
        (
            "decline, no header",
            None,
            Method::POST,
            format!("/v1/backend/contacts/{}/decline", f.alice_id),
            None,
        ),
    ] {
        let body = serde_json::json!({"participant_id": f.alice_id});
        let (status, json, _) =
            request_json_opt(&f.router, method, &path, token, key, Some(&body)).await;
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
        &format!("/v1/backend/contacts/{}", f.alice_id),
        &f.alice_token,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "invalid_credential");
}

// ── 2. Agent 看见并自己应答 ────────────────────────────────────────────────

#[tokio::test]
async fn agent_sees_and_answers_a_human_request() {
    let f = fixture().await;

    // Human → Agent 申请（Human 侧端点，字段名已改名）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        &f.alice_token,
        Some("bc-h2a"),
        Some(&serde_json::json!({"participant_id": f.agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "human requests agent: {json}");
    assert_eq!(json["contact"]["status"], "pending");
    assert_eq!(json["contact"]["peer"]["id"], serde_json::json!(f.agent_a));
    assert_eq!(json["contact"]["peer"]["kind"], "agent");

    // Agent 自己看得见这条申请（BackendAuth）。
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/backend/contacts",
        &f.agent_a_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agent lists: {json}");
    let pending = contact_with(&json, &f.alice_id).expect("the pending request is visible");
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["peer"]["kind"], "human");
    // **方向相对观察者**：这条是对方发起给我的 ⇒ 等我回答（incoming）。主体做决定
    // 需要的信息不该靠试错拿（批次三第三刀 第②步补的字段）。
    assert_eq!(
        pending["direction"], "incoming",
        "the agent must be able to tell which pending request is waiting for its answer: {json}"
    );
    // 人类那一侧看同一条关系：方向相反。
    let (_, human_view, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/contacts",
        &f.alice_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        contact_with(&human_view, &f.agent_a).unwrap()["direction"],
        "outgoing",
        "{human_view}"
    );

    // **Agent 自己接受**（同意归被申请的那一方）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/contacts/{}/accept", f.alice_id),
        &f.agent_a_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agent accepts: {json}");
    assert_eq!(json["contact"]["status"], "accepted");

    // 双方都看到 accepted（同一段事实的两个视角）。
    let (_, agent_view, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/backend/contacts",
        &f.agent_a_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        contact_with(&agent_view, &f.alice_id).unwrap()["status"],
        "accepted"
    );
    let (_, human_view, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/contacts",
        &f.alice_token,
        None,
        None,
    )
    .await;
    let row = contact_with(&human_view, &f.agent_a).expect("human sees the agent contact");
    assert_eq!(row["status"], "accepted");
    assert_eq!(row["peer"]["kind"], "agent");
}

// ── 3. Agent 主动发起，Human 接受 ──────────────────────────────────────────

#[tokio::test]
async fn agent_initiates_and_a_human_accepts() {
    let f = fixture().await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/contacts/requests",
        &f.agent_a_token,
        Some("bc-a2h"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent requests human: {json}");
    assert_eq!(json["contact"]["status"], "pending");

    // 同 key 重放 → 200 replayed；同 key 换目标 → 409。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/contacts/requests",
        &f.agent_a_token,
        Some("bc-a2h"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replay: {json}");
    assert_eq!(json["idempotency_replayed"], true);
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/contacts/requests",
        &f.agent_a_token,
        Some("bc-a2h"),
        Some(&serde_json::json!({"participant_id": f.alice_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // 目标未知 ⇒ 404 participant_not_found（新码，只给这条路径）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/contacts/requests",
        &f.agent_a_token,
        Some("bc-a2ghost"),
        Some(&serde_json::json!({"participant_id": Uuid::new_v4().to_string()})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "participant_not_found");

    // Human 接受（Human 侧端点，peer 是一支 Agent）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", f.agent_a),
        &f.bob_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "human accepts agent: {json}");
    assert_eq!(json["contact"]["status"], "accepted");
}

// ── 4. 解除与"答不了别人的申请" ────────────────────────────────────────────

#[tokio::test]
async fn agent_ends_a_relation_and_cannot_answer_someone_elses_request() {
    let f = fixture().await;
    // Alice 申请 Agent A；Bob 也申请 Agent A（两条独立关系）。
    for (token, key) in [(&f.alice_token, "bc-x1"), (&f.bob_token, "bc-x2")] {
        let (status, json, _) = request_json(
            &f.router,
            Method::POST,
            "/v1/contacts/requests",
            token,
            Some(key),
            Some(&serde_json::json!({"participant_id": f.agent_a})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{json}");
    }
    // Agent A 接受 Alice 的；对 Bob 的那条**它还没答**。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/contacts/{}/accept", f.alice_id),
        &f.agent_a_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    // **Agent B（第三方）答不了别人给 A 的申请**：它不是那条关系的 addressee。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/contacts/{}/accept", f.bob_id),
        &f.agent_b_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "contact_not_found");

    // Agent A 单方面解除与 Alice 的关系 ⇒ 204，列表里不再有这一条。
    let (status, json) = delete(
        &f.router,
        &format!("/v1/backend/contacts/{}", f.alice_id),
        &f.agent_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "agent ends: {json}");
    let (_, view, _) = request_json(
        &f.router,
        Method::GET,
        "/v1/backend/contacts",
        &f.agent_a_token,
        None,
        None,
    )
    .await;
    assert!(
        contact_with(&view, &f.alice_id).is_none(),
        "the closed relation must not be listed: {view}"
    );
    // 幂等：再解除一次仍是 204。
    let (status, _) = delete(
        &f.router,
        &format!("/v1/backend/contacts/{}", f.alice_id),
        &f.agent_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ── 5. 邀请资格随关系走（与第一刀合起来的反证） ────────────────────────────

#[tokio::test]
async fn relations_are_the_invite_qualification_now() {
    let f = fixture().await;

    // (a) Agent 有了一位 accepted 关系的 Human ⇒ **它就能邀请这位 Human 进自己的群**。
    //     第一刀登记的"Agent owner 邀请不到任何人"在这里当场消失。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        &f.alice_token,
        Some("bc-q1"),
        Some(&serde_json::json!({"participant_id": f.agent_a})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/contacts/{}/accept", f.alice_id),
        &f.agent_a_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/backend/groups",
        &f.agent_a_token,
        Some("bc-g1"),
        Some(&serde_json::json!({"name": "甲与主人"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent creates group: {json}");
    let group = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/rooms/{group}/members"),
        &f.agent_a_token,
        Some("bc-g1-i"),
        Some(&serde_json::json!({"participant_id": f.alice_id})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "an Agent owner may now invite its accepted contact: {json}"
    );
    assert_eq!(
        json["membership"]["participant_id"],
        serde_json::json!(f.alice_id)
    );

    // (b) 反向：Human owner 能邀请**自己并不拥有**、但与自己有关系的 Agent。
    //     Agent B 属于 Bob；Alice 与它建立关系后即可邀请。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        &f.alice_token,
        Some("bc-q2"),
        Some(&serde_json::json!({"participant_id": f.agent_b})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/backend/contacts/{}/accept", f.alice_id),
        &f.agent_b_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/groups",
        &f.alice_token,
        Some("bc-g2"),
        Some(&serde_json::json!({"name": "主人的群"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let alice_group = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{alice_group}/members"),
        &f.alice_token,
        Some("bc-g2-i"),
        Some(&serde_json::json!({"participant_id": f.agent_b})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a human owner may invite an agent it does not own but has a relation with: {json}"
    );

    // 没有关系的主体仍然进不来（判据没被放宽成"谁都能进"）：Bob 与 Alice
    // 之间没有任何关系段。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{alice_group}/members"),
        &f.alice_token,
        Some("bc-g2-x"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "member_not_found");
}

// ── 6. Human 侧形状：字段改名，旧名一律 400 ────────────────────────────────

#[tokio::test]
async fn human_contact_request_uses_participant_id_and_rejects_the_old_name() {
    let f = fixture().await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        &f.alice_token,
        Some("bc-shape-1"),
        Some(&serde_json::json!({"participant_id": f.bob_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        &f.alice_token,
        Some("bc-shape-2"),
        Some(&serde_json::json!({"human_id": f.agent_a})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the old field name must be an unknown field: {json}"
    );
    assert_eq!(json["error"]["code"], "invalid_request");
}
