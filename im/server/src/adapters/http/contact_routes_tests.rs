//! Contact HTTP 契约测试（CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md 5.2）：请求
//! 创建（幂等/反向收敛）、列表、接受权限、错误码矩阵与错误信封。

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
use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
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
    token_a: String,
    human_a: String,
    token_b: String,
    human_b: String,
    pool: SqlitePool,
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
    let (human_b, token_b) = add_human(&pool, &identity, "Bob").await;
    let router = build_router(
        Arc::new(store),
        Arc::new(identity.clone()),
        Arc::new(agent),
        Arc::new(room),
        Arc::new(SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(SqliteEventStore::new(pool.clone())),
        Arc::new(SqliteContactStore::new(pool.clone())),
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
        human_b,
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

async fn post_contact(
    router: &axum::Router,
    token: &str,
    key: &str,
    human_id: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, json, _request_id) = send(
        router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(
            serde_json::json!({"participant_id": human_id})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    (status, json)
}

#[tokio::test]
async fn contact_request_flow_accept_and_reverse_convergence() {
    let f = fixture().await;
    let (status, json) = post_contact(&f.router, &f.token_a, "k-ab", &f.human_b).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "first request must be 201: {json}"
    );
    assert_eq!(json["contact"]["status"], "pending");
    assert_eq!(json["contact"]["peer"]["id"], f.human_b);
    assert_eq!(json["idempotency_replayed"], false);

    // 同 key 幂等重放 → 200 同一关系。
    let (status, json) = post_contact(&f.router, &f.token_a, "k-ab", &f.human_b).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["contact"]["status"], "pending");

    // 反向请求（B→A）→ 200 同一 pending 关系，requester 不翻转。
    let (status, json) = post_contact(&f.router, &f.token_b, "k-ba", &f.human_a).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reverse request returns existing: {json}"
    );
    assert_eq!(json["idempotency_replayed"], true);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_periods")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "one symmetric active period per pair");

    // 同 key 换目标 → 409 idempotency_conflict。
    let ghost = Uuid::new_v4().to_string();
    let (status, json) = post_contact(&f.router, &f.token_a, "k-ab", &ghost).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // requester 不能接受自己的请求。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", f.human_b),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "requester accept must fail: {json}"
    );
    assert_eq!(json["error"]["code"], "contact_not_found");

    // addressee 接受 → 200 accepted；重复接受幂等。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", f.human_a),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["contact"]["status"], "accepted");
    assert!(json["contact"]["accepted_at"].is_string());
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{}/accept", f.human_a),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-accept is idempotent: {json}");
    assert_eq!(json["contact"]["status"], "accepted");

    // 无关系对接受 → 404。
    let ghost = Uuid::new_v4().to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{ghost}/accept"),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "contact_not_found");
}

#[tokio::test]
async fn contact_list_shows_pending_and_accepted_states() {
    let f = fixture().await;
    post_contact(&f.router, &f.token_a, "k-ab", &f.human_b).await;
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/contacts",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["contacts"].as_array().unwrap().len(), 1);
    assert_eq!(json["contacts"][0]["status"], "pending");
    assert_eq!(json["contacts"][0]["peer"]["id"], f.human_b);
    assert!(json["contacts"][0].get("idempotency").is_none());

    // B 视角也看到同一条关系（对称）。
    let (_, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/contacts",
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(json["contacts"].as_array().unwrap().len(), 1);
    assert_eq!(json["contacts"][0]["peer"]["id"], f.human_a);
}

#[tokio::test]
async fn contact_request_error_matrix() {
    let f = fixture().await;

    // 未知目标 → 404 participant_not_found（与 bad-id 同一码；批次三第二刀由
    // human_not_found 改名——目标从此可以是任意 active 主体）。
    let ghost = Uuid::new_v4().to_string();
    let (status, json) = post_contact(&f.router, &f.token_a, "k-1", &ghost).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "participant_not_found");
    let (status, json) = post_contact(&f.router, &f.token_a, "k-2", "not-a-uuid").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "participant_not_found");

    // 自我请求 → 400 invalid_request。
    let (status, json) = post_contact(&f.router, &f.token_a, "k-3", &f.human_a).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    // 缺 key / 非法 key。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json")],
        Some(
            serde_json::json!({"participant_id": f.human_b})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "idempotency_key_required");
    let (status, json) = post_contact(&f.router, &f.token_a, "a,b", &f.human_b).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_idempotency_key");

    // 未知字段 → 400 invalid_request。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "k-4"), ("content-type", "application/json")],
        Some(
            serde_json::json!({"participant_id": f.human_b, "extra": 1})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    // 无认证 → 401 authentication_required；无效 Session → 401 invalid_session。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        None,
        &[(KEY_HEADER, "k-5"), ("content-type", "application/json")],
        Some(
            serde_json::json!({"participant_id": f.human_b})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");
    let (status, json) = post_contact(&f.router, "not-a-token", "k-6", &f.human_b).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");

    // 失败不留下联系人事实。
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_periods")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "failed requests must not create contact facts");
}
