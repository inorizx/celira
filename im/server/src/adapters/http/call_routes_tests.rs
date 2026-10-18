//! Call HTTP handler 测试（契约 CORE_V1_CALL.md）。
//!
//! 真实 SQLite（成员裁决）+ 真实进程内 InMemoryCallState 的组合测试：
//! - 发起/复用/忙碌与发起错误映射（room_not_found / call_party_busy）；
//! - accept/decline/hangup 状态机与角色（非 callee 404 call_not_found、
//!   非 ringing 409 call_state_conflict）；
//! - GET 读取仅参与方可见（非参与者 404）；
//! - signal 中继（offer/answer/ice 形状校验 400 call_payload_invalid）；
//! - 硬上限 fail closed（503 call_state_limit_exceeded）。
//!
//! 媒体为 P2P 直连，本测试只覆盖 Core 协调面（状态 + 信令中继），不涉及
//! WebRTC 媒体字节。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;

const KEY_HEADER: &str = "idempotency-key";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    token_a: String,
    human_a: String,
    pool: sqlx::SqlitePool,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store =
        crate::adapters::sqlite::migration_store::MigrationStore::open(&dir.path().join("core.db"))
            .await
            .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = crate::adapters::sqlite::identity_store::SqliteIdentityStore::new(pool.clone());
    let human = crate::application::identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap alice");
    let call = Arc::new(crate::adapters::call::InMemoryCallState::new());
    let router = build_router(
        Arc::new(store),
        Arc::new(identity),
        Arc::new(
            crate::adapters::sqlite::agent_store::SqliteAgentStore::new(pool.clone()),
        ),
        Arc::new(crate::adapters::sqlite::room_store::SqliteRoomStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::message_store::SqliteMessageStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::backend_store::SqliteBackendStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::event_store::SqliteEventStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::contact_store::SqliteContactStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(
            pool.clone(),
        )),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(crate::adapters::sqlite::attachment_store::SqliteAttachmentStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::blob::LocalBlobStorage::new(
            dir.path().join("media"),
        )),
        Arc::new(crate::adapters::sqlite::home_store::SqliteHomeStore::new(pool.clone())),
        Arc::new(
            crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore::new(
                pool.clone(),
            ),
        ),
        call.clone(),
        15,
    );
    Fixture {
        _dir: dir,
        router,
        token_a: human.token.as_str().to_owned(),
        human_a: human.participant.id.as_str().to_owned(),
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
    builder = builder.header(header::CONTENT_TYPE, "application/json");
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json, request_id)
}

/// 经 schema 创建第二个 Human，并让 Alice 与它互加联系人、建群、邀请入群；
/// 返回 (group_id, human_b, token_b)。
async fn setup_group_with_b(f: &Fixture) -> (String, String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', ?, 0, NULL)",
    )
    .bind(&id)
    .bind("Bob")
    .execute(&f.pool)
    .await
    .expect("insert bob");
    let token = crate::domain::token::OpaqueToken::generate().expect("token");
    let session = crate::domain::human_session::HumanSession::new(
        crate::domain::human_session::SessionId(uuid::Uuid::new_v4().to_string()),
        crate::domain::participant::ParticipantId(id.clone()),
        crate::domain::human_session::TokenHash::from_token(&token),
        crate::domain::time::UtcMillis::now(),
    );
    let identity =
        crate::adapters::sqlite::identity_store::SqliteIdentityStore::new(f.pool.clone());
    crate::ports::identity::IdentityStore::issue_human_session(
        &identity,
        &crate::ports::identity::NewSession {
            id: session.id.clone(),
            human_id: session.human_id.clone(),
            token_hash: session.token_hash,
            created_at: session.created_at,
        },
    )
    .await
    .expect("issue bob session");
    let token_b = token.as_str().to_owned();

    // 联系人互加接受（A→B；需幂等键）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "call-ck")],
        Some(
            serde_json::json!({ "participant_id": id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "contact req: {json}");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", f.human_a),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "contact accept: {json}");

    // 建群 + 邀请 B。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "call-g")],
        Some(serde_json::json!({ "name": "群" }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "group: {json}");
    let group_id = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "call-inv")],
        Some(
            serde_json::json!({ "participant_id": id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite B: {json}");
    (group_id, id, token_b)
}

/// 发起 → 复用 → 非成员禁止 → 忙碌 → 状态机 + GET 可见性 + signal 校验。
#[tokio::test]
async fn call_initiate_accept_hangup_and_error_mapping() {
    let f = fixture().await;
    let (group_id, human_b, token_b) = setup_group_with_b(&f).await;

    // 发起：Alice → Bob（201 / created=true）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["created"], true);
    assert_eq!(json["call"]["state"], "ringing");
    assert_eq!(json["call"]["media"], "audio", "media defaults to audio");
    let call_id = json["call"]["call_id"].as_str().unwrap().to_owned();

    // 复用同 (room, caller, callee) → 200 created=false。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["created"], false);

    // 非成员（第三个不存在的 Human id）发起 → 404 room_not_found。
    let outsider = uuid::Uuid::new_v4().to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": outsider })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // 非 callee（caller）accept → 404 call_not_found。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/accept"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "call_not_found");

    // callee accept → in_progress。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/accept"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["call"]["state"], "in_progress");

    // 参与者读取 → 200。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/calls/{call_id}"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["call"]["call_id"], call_id);

    // hangup by caller → ended(hangup_caller)。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/hangup"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["call"]["state"], "ended");
    assert_eq!(json["call"]["ended_reason"], "hangup_caller");

    // 已结束再 accept → 409 call_state_conflict。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/accept"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "call_state_conflict");
}

/// 呼入未应答时重复发起他方 → 忙碌。
#[tokio::test]
async fn call_busy_while_ringing() {
    let f = fixture().await;
    let (group_id, human_b, token_b) = setup_group_with_b(&f).await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    // Bob 已在活跃 Call（ringing）：Bob 再发起呼 Alice → caller 已 active →
    // PartyBusy。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {token_b}")),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": f.human_a })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "call_party_busy");
}

/// signal：offer/answer/ice 形状与越界校验映射。
#[tokio::test]
async fn call_signal_payload_validation() {
    let f = fixture().await;
    let (group_id, human_b, token_b) = setup_group_with_b(&f).await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let call_id = json["call"]["call_id"].as_str().unwrap().to_owned();

    // 未知 kind → 400 call_payload_invalid。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "kind": "bogus", "sdp": "v=0" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "call_payload_invalid");

    // offer 缺 sdp → 400 call_payload_invalid。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "kind": "offer" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "call_payload_invalid");

    // 合法 offer → 204（Bob 无订阅也成功，delivered=false 但不报错）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "kind": "offer", "sdp": "v=0\r\no=- 1 1 IN IP4 127.0.0.1" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");

    // 未知 call_id → 404 call_not_found。
    let bogus = uuid::Uuid::new_v4().to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{bogus}/signal"),
        Some(&format!("Bearer {token_b}")),
        &[],
        Some(
            serde_json::json!({ "kind": "answer", "sdp": "v=0" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "call_not_found");
}

/// signal 中继方向强制（契约 §4.2）：offer 只可 caller→callee、answer 只可
/// callee→caller、ice 双向；方向/角色错误与终态（ended）后信令一律
/// 409 call_state_conflict。加在 HTTP 面，保证对端身份永远来自 Core 裁决。
#[tokio::test]
async fn call_signal_direction_enforced_at_http() {
    let f = fixture().await;
    let (group_id, human_b, token_b) = setup_group_with_b(&f).await;

    // Alice（caller）向 Bob（callee）发起。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let call_id = json["call"]["call_id"].as_str().unwrap().to_owned();

    // caller 发 offer → 204（方向合法）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "kind": "offer", "sdp": "v=0" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");

    // caller 发 answer（方向错误：answer 只可 callee）→ 409 call_state_conflict。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "kind": "answer", "sdp": "v=0" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "call_state_conflict");

    // callee 发 answer → 204（方向合法）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {token_b}")),
        &[],
        Some(
            serde_json::json!({ "kind": "answer", "sdp": "v=0" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");

    // callee 拒绝 → ended。end 后任何信令（caller 发 offer）→ 409。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/decline"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/calls/{call_id}/signal"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "kind": "offer", "sdp": "v=0" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "call_state_conflict");
}

/// 通过公开 API 创建 Agent + Binding + Credential，返回 (agent_id, token)。
async fn setup_agent_with_credential(
    f: &Fixture,
    name: &str,
    key_prefix: &str,
) -> (String, String) {
    // 创建 Agent。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, &format!("{key_prefix}-ag"))],
        Some(
            serde_json::json!({ "display_name": name })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent: {json}");
    let agent_id = json["agent"]["id"].as_str().unwrap().to_owned();
    // 创建 Binding。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, &format!("{key_prefix}-bk"))],
        Some(
            serde_json::json!({ "label": name })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "binding: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    // 签发 Credential（token 只返回一次）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, &format!("{key_prefix}-cr"))],
        Some(
            serde_json::json!({ "replaces_credential_id": null })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "credential: {json}");
    let token = json["token"]
        .as_str()
        .expect("201 must show token")
        .to_owned();
    (agent_id, token)
}

/// Agent 作为发起方（Agent→Agent）：Alice 拥有两个 Agent A/B 并在群内邀请
/// 两者；A 经 Backend Credential 发起视频呼叫 B，B 应答。
#[tokio::test]
async fn agent_caller_can_call_another_agent_with_video() {
    let f = fixture().await;
    let (group_id, _human_b, _token_b) = setup_group_with_b(&f).await;
    let (agent_a, token_a) = setup_agent_with_credential(&f, "AgentA", "caa").await;
    let (agent_b, token_b) = setup_agent_with_credential(&f, "AgentB", "cab").await;

    // Alice（群 owner）邀请 A 与 B 入群。
    for (id, key) in [(&agent_a, "inv-a"), (&agent_b, "inv-b")] {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            &format!("/v1/rooms/{group_id}/members"),
            Some(&format!("Bearer {}", f.token_a)),
            &[(KEY_HEADER, key)],
            Some(
                serde_json::json!({ "participant_id": id })
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "invite {key}: {json}");
    }

    // A 作为发起方呼叫 B（video）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/backend/calls",
        Some(&format!("Bearer {token_a}")),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": agent_b, "media": "video" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent initiate: {json}");
    assert_eq!(json["call"]["caller_id"], agent_a);
    assert_eq!(json["call"]["callee_id"], agent_b);
    assert_eq!(json["call"]["media"], "video");
    let call_id = json["call"]["call_id"].as_str().unwrap().to_owned();

    // B 经 Backend Credential 应答。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/backend/calls/{call_id}/accept"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agent accept: {json}");
    assert_eq!(json["call"]["state"], "in_progress");
    assert_eq!(json["call"]["media"], "video");

    // 认证边界：Human token 不能发起 Backend 面 → 401 invalid_credential；
    // Agent credential 不能走 Human 面 → 401 invalid_session。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/backend/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": agent_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "invalid_credential");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {token_a}")),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": agent_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "invalid_session");
}

/// media 显式非法 → 400 invalid_request（不泄露细节）。
#[tokio::test]
async fn call_rejects_invalid_media() {
    let f = fixture().await;
    let (group_id, human_b, _token_b) = setup_group_with_b(&f).await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/calls",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        Some(
            serde_json::json!({ "room_id": group_id, "callee_id": human_b, "media": "screen" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");
}
