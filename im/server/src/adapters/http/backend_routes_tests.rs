//! Backend HTTP 契约测试（CORE_V1_BACKEND_BINDINGS.md 5.x/6.x/7.x）：Binding
//! 创建/幂等/列表/读取/撤销、Credential 签发/CAS 轮换/撤销、Backend 认证
//! 通道隔离、错误信封、token 脱敏与状态码矩阵。

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
use crate::domain::backend::BackendToken;
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::identity::{IdentityStore as _, NewSession};
use crate::protocol::backend::{CredentialView, IssueCredentialResponse};

const KEY_HEADER: &str = "idempotency-key";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    pool: SqlitePool,
    token_a: String,
    human_a_id: String,
    token_b: String,
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
    Fixture {
        _dir: dir,
        router,
        pool,
        token_a: a.token.as_str().to_owned(),
        human_a_id: a.participant.id.as_str().to_owned(),
        token_b,
    }
}

impl Fixture {
    fn human_a_id(&self) -> &str {
        &self.human_a_id
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

/// 通过公开 API 创建 Agent，返回 agent_id。
async fn create_agent(router: &axum::Router, token: &str) -> String {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        "/v1/agents",
        token,
        Some("a-1"),
        Some(&serde_json::json!({"display_name": "助手"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    json["agent"]["id"].as_str().unwrap().to_owned()
}

/// 通过公开 API 创建 Binding，返回 binding_id。
async fn create_binding(
    router: &axum::Router,
    token: &str,
    agent_id: &str,
    label: &str,
    key: &str,
) -> String {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        token,
        Some(key),
        Some(&serde_json::json!({"label": label})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    json["binding"]["id"].as_str().unwrap().to_owned()
}

/// 通过公开 API 签发 Credential，返回 (credential_id, token)。
async fn issue_credential(
    router: &axum::Router,
    token: &str,
    agent_id: &str,
    binding_id: &str,
    replaces: Option<&str>,
    key: &str,
) -> (String, String) {
    let body = serde_json::json!({
        "replaces_credential_id": replaces,
    });
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        token,
        Some(key),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    let credential_id = json["credential"]["id"].as_str().unwrap().to_owned();
    let token = json["token"]
        .as_str()
        .expect("201 must show token")
        .to_owned();
    (credential_id, token)
}

async fn backend_me(
    router: &axum::Router,
    token: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let (status, json, request_id) = send(
        router,
        Method::GET,
        "/v1/backend/me",
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    (status, json, request_id)
}

#[tokio::test]
async fn binding_create_replay_and_conflict() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let path = format!("/v1/agents/{agent_id}/backend-bindings");

    // 首次 201：无 token/hash/Room/Runtime 字段。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("b-1"),
        Some(&serde_json::json!({"label": "local runtime"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(json["binding"]["label"], "local runtime");
    assert_eq!(json["binding"]["agent_id"], agent_id);
    assert_eq!(json["binding"]["created_by_human_id"], f.human_a_id());
    assert_eq!(json["binding"]["status"], "active");
    assert!(json["binding"]["revoked_at"].is_null());
    assert!(json["binding"].get("token").is_none());
    assert!(json["binding"].get("token_hash").is_none());
    assert!(json["binding"].get("idempotency_key").is_none());
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    assert!(uuid::Uuid::parse_str(&binding_id).is_ok());

    // 重放 200：同 ID、replayed=true、不新增行。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("b-1"),
        Some(&serde_json::json!({"label": "local runtime"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["binding"]["id"], binding_id);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_bindings")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // 同 key 不同 label → 409，首次事实不变。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("b-1"),
        Some(&serde_json::json!({"label": "other"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_bindings")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // 第二个 active → 409 backend_binding_exists。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("b-2"),
        Some(&serde_json::json!({"label": "second"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "backend_binding_exists");
}

#[tokio::test]
async fn binding_owner_scope_and_validation_errors() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let path = format!("/v1/agents/{agent_id}/backend-bindings");

    // 非 Owner → 404 agent_not_found（不泄露存在性）。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_b,
        Some("b-x"),
        Some(&serde_json::json!({"label": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "agent_not_found");

    // unknown agent id → 404 agent_not_found；bad id → 404。
    for ghost in [Uuid::new_v4().to_string(), "not-a-uuid".to_owned()] {
        let (status, json, _) = request_json(
            &f.router,
            Method::POST,
            &format!("/v1/agents/{ghost}/backend-bindings"),
            &f.token_a,
            Some("b-y"),
            Some(&serde_json::json!({"label": "x"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "ghost {ghost}");
        assert_eq!(json["error"]["code"], "agent_not_found");
    }

    // 缺失 Idempotency-Key → 400；非法 key → 400。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &path,
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json")],
        Some(br#"{"label":"x"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "idempotency_key_required");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("has,comma"),
        Some(&serde_json::json!({"label": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_idempotency_key");

    // 非法 label：空 → 400 invalid_request；超 80 scalar → 400；NUL → 400。
    for label in ["", "  ", &"a".repeat(81)] {
        let (status, json, _) = request_json(
            &f.router,
            Method::POST,
            &path,
            &f.token_a,
            Some("b-z"),
            Some(&serde_json::json!({"label": label})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "label {label:?}");
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    // 超限 body → 413 payload_too_large。
    let oversized = serde_json::json!({"label": "x".repeat(5000)}).to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &path,
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "b-big"), ("content-type", "application/json")],
        Some(oversized.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json["error"]["code"], "payload_too_large");

    // revoked Agent → 409 agent_not_active。
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
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("b-after-revoke"),
        Some(&serde_json::json!({"label": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "agent_not_active");
}

#[tokio::test]
async fn binding_list_get_revoke() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let b1 = create_binding(&f.router, &f.token_a, &agent_id, "one", "b-1").await;
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{b1}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "revoke must be 204");
    let b2 = create_binding(&f.router, &f.token_a, &agent_id, "two", "b-2").await;

    // 列表包含 revoked 历史，按创建时间升序。
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        &f.token_a,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = json["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![b1.as_str(), b2.as_str()]);
    assert_eq!(json["bindings"][0]["status"], "revoked");
    assert_eq!(json["bindings"][1]["status"], "active");

    // 单个读取；unknown/bad/not-under-agent → 404 backend_binding_not_found。
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/backend-bindings/{b2}"),
        &f.token_a,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["binding"]["id"], b2);
    for ghost in [Uuid::new_v4().to_string(), "bad-id".to_owned()] {
        let (status, json, _) = request_json(
            &f.router,
            Method::GET,
            &format!("/v1/agents/{agent_id}/backend-bindings/{ghost}"),
            &f.token_a,
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "ghost {ghost}");
        assert_eq!(json["error"]["code"], "backend_binding_not_found");
    }

    // 重复撤销 → 204；未知 binding → 404。
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{b1}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "repeat revoke must be 204");
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{b2}"),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "non-owner revoke");
    assert_eq!(json["error"]["code"], "agent_not_found");
}

#[tokio::test]
async fn credential_issue_shows_token_once_and_replay_returns_null() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let path = format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials");

    // 首次签发 201：唯一一次返回原始 token。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("c-1"),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    assert_eq!(json["idempotency_replayed"], false);
    let token = json["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("clr_bc_"), "token format");
    assert_eq!(token.len(), 7 + 43, "token length");
    assert!(BackendToken::parse_candidate(&token).is_some());
    let credential_id = json["credential"]["id"].as_str().unwrap().to_owned();
    assert_eq!(json["credential"]["binding_id"], binding_id);
    assert_eq!(json["credential"]["status"], "active");
    assert!(json["credential"].get("token_hash").is_none());
    assert!(json["credential"].get("idempotency_key").is_none());
    assert!(json["credential"].get("issued_by_human_id").is_none());

    // 数据库只有 hash，无明文。
    let stored: String =
        sqlx::query_scalar("SELECT token_hash FROM backend_credentials WHERE id = ?")
            .bind(&credential_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(stored.len(), 64);
    assert_ne!(stored, token);
    assert!(!stored.contains("clr_bc_"));
    let plaintext: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials WHERE token_hash = ?")
            .bind(&token)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(plaintext, 0);

    // 重放 200：token=null，同 Credential 元数据，不新增。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("c-1"),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert!(json["token"].is_null(), "replay must not re-show the token");
    assert_eq!(json["credential"]["id"], credential_id);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // 同 key 换 replaces → 409 idempotency_conflict。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("c-1"),
        Some(&serde_json::json!({"replaces_credential_id": credential_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // 首次签发时已有 active → 409 credential_state_conflict。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("c-2"),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "credential_state_conflict");

    // 缺失字段 / 非 UUID replaces → 400 invalid_request。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("c-3"),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &path,
        &f.token_a,
        Some("c-4"),
        Some(&serde_json::json!({"replaces_credential_id": "not-a-uuid"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn credential_cas_rotation_over_http() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (c1, token1) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    // 轮换：指定当前 active ID → 201 新 token。
    let active_id = get_credential_list(&f, &agent_id, &binding_id).await;
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        &f.token_a,
        Some("c-2"),
        Some(&serde_json::json!({"replaces_credential_id": active_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    let token2 = json["token"].as_str().unwrap().to_owned();

    // 旧 token 立即失效，新 token 生效。
    let (status, _, _) = backend_me(&f.router, &token1).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, json, _) = backend_me(&f.router, &token2).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent"]["id"], agent_id);
    assert_eq!(json["binding"]["id"], binding_id);
    assert_eq!(json["agent"]["kind"], "agent");

    // stale replaces（已轮换掉的旧 ID）→ 409 credential_state_conflict。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        &f.token_a,
        Some("c-3"),
        Some(&serde_json::json!({"replaces_credential_id": c1})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "credential_state_conflict");
    // 轮换不恢复旧 token。
    let (status, _, _) = backend_me(&f.router, &token1).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// 读取 active Credential ID（模拟“丢失签发响应”后的状态解析）。
async fn get_credential_list(f: &Fixture, agent_id: &str, binding_id: &str) -> String {
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        &f.token_a,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json["credentials"][0].get("token").is_none(),
        "no token in list"
    );
    assert!(json["credentials"][0].get("token_hash").is_none());
    json["credentials"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["status"] == "active")
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .unwrap()
}

#[tokio::test]
async fn backend_me_channel_isolation_and_fail_closed() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (credential_id, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    // 成功：只返回 agent/binding/credential 元数据。
    let (status, json, request_id) = backend_me(&f.router, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent"]["id"], agent_id);
    assert_eq!(json["agent"]["kind"], "agent");
    assert_eq!(json["agent"]["status"], "active");
    assert_eq!(json["binding"]["id"], binding_id);
    assert_eq!(json["binding"]["label"], "local runtime");
    assert_eq!(json["credential"]["id"], credential_id);
    assert_eq!(json["credential"]["status"], "active");
    assert!(request_id.is_some());
    for field in [
        "owner",
        "room",
        "membership",
        "permission",
        "event",
        "token",
        "token_hash",
    ] {
        assert!(
            json.get(field).is_none(),
            "backend/me must not expose {field}"
        );
        assert!(json["agent"].get(field).is_none());
    }

    // 通道隔离：Human token 不能调 backend/me → 401 invalid_credential。
    let (status, json, _) = backend_me(&f.router, &f.token_a).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_credential");

    // Backend token 不能调 Human/Agent/Room/Message 端点 → 401。
    let (status, json, _) =
        request_json(&f.router, Method::GET, "/v1/me", &token, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");
    let (status, _, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/agents",
        &token,
        Some("k"),
        Some(&serde_json::json!({"display_name": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = request_json(
        &f.router,
        Method::POST,
        "/v1/agents/{agent_id}/backend-bindings",
        &token,
        Some("k"),
        Some(&serde_json::json!({"label": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 缺 header → 401 authentication_required；格式错误 → 401 invalid_credential。
    let (status, json, _) = send(&f.router, Method::GET, "/v1/backend/me", None, &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");
    for bad in [
        "definitely-not-a-token",
        "clr_hs_0000000000000000000000000000000000000000000",
        &format!("clr_bc_{}", "a".repeat(42)),
    ] {
        let (status, json, _) = backend_me(&f.router, bad).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "bad token {bad:?}");
        assert_eq!(json["error"]["code"], "invalid_credential");
    }
    // 未知 token（格式合法）→ 同一 401 与同一错误文案。
    let unknown = BackendToken::generate().unwrap().as_str().to_owned();
    let (status, json, _) = backend_me(&f.router, &unknown).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["message"], "invalid backend credential");
}

#[tokio::test]
async fn revoke_cascade_fail_closed_matrix() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (c1, token1) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;
    let (c2, token2) = issue_credential(
        &f.router,
        &f.token_a,
        &agent_id,
        &binding_id,
        Some(&c1),
        "c-2",
    )
    .await;
    let _ = c2;

    // revoke Credential → 立即 401；Binding/Agent 保持 active。
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials/{c1}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = backend_me(&f.router, &token1).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked credential must fail"
    );
    let (status, json, _) = backend_me(&f.router, &token2).await;
    assert_eq!(status, StatusCode::OK, "other credential unaffected");
    assert_eq!(json["credential"]["id"], c2);
    // 重复撤销 → 204 且不改 revoked_at。
    let revoked_at: i64 =
        sqlx::query_scalar("SELECT revoked_at FROM backend_credentials WHERE id = ?")
            .bind(&c1)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials/{c1}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let revoked_at_again: i64 =
        sqlx::query_scalar("SELECT revoked_at FROM backend_credentials WHERE id = ?")
            .bind(&c1)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(revoked_at, revoked_at_again);

    // revoke Binding → 同事务级联 Credential；旧 token 立即 401。
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = backend_me(&f.router, &token2).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let cred_statuses: Vec<String> = sqlx::query_scalar("SELECT status FROM backend_credentials")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    assert!(cred_statuses.iter().all(|s| s == "revoked"));

    // revoke Agent → Binding + Credential 同事务 revoked；旧 token 401。
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
    let binding_status: String = sqlx::query_scalar("SELECT status FROM backend_bindings")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(binding_status, "revoked");

    // 撤销后不可恢复：再签发 → 409/404 级别拒绝。
    let (status, json, _) = request_json(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        &f.token_a,
        Some("c-9"),
        Some(&serde_json::json!({"replaces_credential_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "credential_state_conflict");
}

#[tokio::test]
async fn errors_never_leak_token_or_hash() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a).await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (_, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    // 撤销后 backend/me 401 的错误体不得包含 token 或其 hash。
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!(
            "/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials/{cred_id}",
            cred_id = get_credential_list(&f, &agent_id, &binding_id).await
        ),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json, _) = backend_me(&f.router, &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let body = json.to_string();
    assert!(
        !body.contains(&token),
        "error body must not contain the token"
    );
    assert!(
        !body.contains("clr_bc_"),
        "error body must not contain token prefix"
    );

    // 管理响应全文不含 token 前缀；不包含任何 hash 字段。
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        &f.token_a,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = json.to_string();
    assert!(
        !body.contains("clr_bc_"),
        "management response leaks token prefix"
    );
    assert!(!body.contains("token_hash"));
}

#[test]
fn issue_credential_response_debug_redacts_token() {
    let fake_token = "clr_bc_FAKE_DO_NOT_USE_AS_A_CREDENTIAL".to_owned();
    let response = IssueCredentialResponse {
        credential: CredentialView {
            id: "00000000-0000-4000-8000-000000000001".into(),
            binding_id: "00000000-0000-4000-8000-000000000002".into(),
            status: "active".into(),
            created_at: "1970-01-01T00:00:00Z".into(),
            revoked_at: None,
        },
        token: Some(fake_token.clone()),
        idempotency_replayed: false,
        request_id: "00000000-0000-4000-8000-000000000004".into(),
    };
    let debug = format!("{response:?}");
    assert!(!debug.contains(&fake_token));
    assert!(debug.contains("[REDACTED]"));
}
