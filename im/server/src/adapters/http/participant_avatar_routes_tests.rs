//! Avatar HTTP 契约测试（CORE_V1_AVATAR_P0.md §3–§4）：Owner 写路径
//! （PUT multipart / DELETE JSON）的幂等/base_version/stale、失败面（SVG 400、
//! 超 2 MiB 413、非 Owner 404、revoked 409）、字节读路径（round-trip、
//! nosniff、非成员/无头像 404）与既有 Profile / member-presentations 投影
//! 的 avatar 存在性。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore;
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
    token_b: String,
}

async fn add_human(
    pool: &SqlitePool,
    identity: &SqliteIdentityStore,
    display_name: &str,
) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
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
    (id, token.as_str().to_owned())
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let a = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap alice");
    let (_human_b, token_b) = add_human(&pool, &identity, "Bob").await;
    let router = build_router(
        Arc::new(store),
        Arc::new(identity.clone()),
        Arc::new(SqliteAgentStore::new(pool.clone())),
        Arc::new(SqliteRoomStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::message_store::SqliteMessageStore::new(pool.clone())),
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
        Arc::new(SqliteParticipantPresentationStore::new(pool.clone())),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    Fixture {
        _dir: dir,
        router,
        token_a: a.token.as_str().to_owned(),
        token_b,
    }
}

struct RawResponse {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl RawResponse {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::json!({}))
    }
}

async fn send_raw(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> RawResponse {
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
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    RawResponse {
        status,
        headers,
        body: body.to_vec(),
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
    let resp = send_raw(router, method, path, authorization, headers, body).await;
    let request_id = resp
        .headers
        .get(crate::adapters::http::middleware::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (resp.status, resp.json(), request_id)
}

async fn create_agent(router: &axum::Router, token: &str, display_name: &str, key: &str) -> String {
    let body = format!(r#"{{"display_name":"{display_name}"}}"#);
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

async fn create_group(router: &axum::Router, token: &str, name: &str, key: &str) -> String {
    let body = format!(r#"{{"name":"{name}"}}"#);
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    json["room"]["id"].as_str().unwrap().to_owned()
}

async fn invite_member(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    participant_id: &str,
    key: &str,
) {
    let body = format!(r#"{{"participant_id":"{participant_id}"}}"#);
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/members"),
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{}", json);
}

/// 构造严格形状的 avatar multipart 体：`base_version` 字段 + `file` 文件 part。
fn multipart_body(
    boundary: &str,
    base_version: &str,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"base_version\"\r\n\r\n{base_version}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

const BOUNDARY: &str = "----avatar-test-boundary";

const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0aIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\x0d\x0a\x2d\xb4\x00\x00\x00\x00IEND\xaeB`\x82";

async fn put_avatar(
    router: &axum::Router,
    token: &str,
    agent_id: &str,
    key: &str,
    base_version: i64,
    bytes: &[u8],
    content_type: &str,
) -> RawResponse {
    let body = multipart_body(
        BOUNDARY,
        &base_version.to_string(),
        "avatar.png",
        content_type,
        bytes,
    );
    let key = key.to_owned();
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    send_raw(
        router,
        Method::PUT,
        &format!("/v1/agents/{agent_id}/profile/avatar"),
        Some(&format!("Bearer {token}")),
        &[
            (KEY_HEADER, key.as_str()),
            ("content-type", content_type.as_str()),
        ],
        Some(body),
    )
    .await
}

#[tokio::test]
async fn avatar_put_lifecycle_and_fail_closed() {
    let fx = fixture().await;
    let agent_id = create_agent(&fx.router, &fx.token_a, "助手", "a-1").await;
    let avatar_path = format!("/v1/agents/{agent_id}/profile/avatar");

    // 首次 PUT → 201（版本 0 → 1），profile.avatar 投影存在。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-1",
        0,
        PNG_BYTES,
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::CREATED, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(
        resp.json()["profile"]["avatar"]["content_type"],
        "image/png"
    );
    assert_eq!(
        resp.json()["profile"]["avatar"]["size_bytes"],
        PNG_BYTES.len() as i64
    );
    assert_eq!(resp.json()["idempotency_replayed"], false);

    // 更新（新 key、base=1）→ 200 版本 2。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-2",
        1,
        PNG_BYTES,
        "image/jpeg",
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 2);
    assert_eq!(
        resp.json()["profile"]["avatar"]["content_type"],
        "image/jpeg"
    );

    // 同 key 同摘要重放首次：即使 head 已到 2，也返回首次成功结果（版本 1）。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-1",
        0,
        PNG_BYTES,
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::CREATED, "replay: {}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(resp.json()["idempotency_replayed"], true);

    // 同 key 不同请求摘要 → 409 idempotency_conflict。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-1",
        0,
        b"different-bytes",
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");

    // 新 key + 过期 base_version → 409 profile_stale。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-3",
        0,
        PNG_BYTES,
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "profile_stale");

    // 非 owner（Bob）→ 404 agent_not_found（存在性隐藏）。
    let resp = put_avatar(
        &fx.router,
        &fx.token_b,
        &agent_id,
        "av-x",
        0,
        PNG_BYTES,
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "agent_not_found");

    // 缺 Idempotency-Key → idempotency_key_required；无认证 → 401。
    let body = multipart_body(BOUNDARY, "2", "avatar.png", "image/png", PNG_BYTES);
    let (status, json, _) = send(
        &fx.router,
        Method::PUT,
        &avatar_path,
        Some(&format!("Bearer {}", fx.token_a)),
        &[(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}").as_str(),
        )],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_key_required");
    let body = multipart_body(BOUNDARY, "2", "avatar.png", "image/png", PNG_BYTES);
    let (status, json, _) = send(
        &fx.router,
        Method::PUT,
        &avatar_path,
        None,
        &[
            (KEY_HEADER, "av-na"),
            (
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}").as_str(),
            ),
        ],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");

    // 撤销 Agent → PUT 409 agent_not_active（授权先于重放 fail closed）。
    let del = send(
        &fx.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(del.0, StatusCode::NO_CONTENT);
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-rev",
        0,
        PNG_BYTES,
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "agent_not_active");
}

#[tokio::test]
async fn avatar_put_rejects_bad_shapes_and_sizes() {
    let fx = fixture().await;
    let agent_id = create_agent(&fx.router, &fx.token_a, "助手", "a-1").await;
    let avatar_path = format!("/v1/agents/{agent_id}/profile/avatar");

    // SVG（可执行类型）→ 400 avatar_invalid。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "k-svg",
        0,
        b"<svg/>",
        "image/svg+xml",
    )
    .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "avatar_invalid");

    // 空字节 → 400 avatar_invalid。
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "k-empty",
        0,
        b"",
        "image/png",
    )
    .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "avatar_invalid");

    // 超 2 MiB → 413 payload_too_large。
    let big = vec![0u8; 2 * 1024 * 1024 + 1];
    let resp = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "k-big",
        0,
        &big,
        "image/png",
    )
    .await;
    assert_eq!(
        resp.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "payload_too_large");

    // 畸形 multipart：缺 file part → 400 avatar_invalid。
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"base_version\"\r\n\r\n0\r\n--{BOUNDARY}--\r\n"
    )
    .into_bytes();
    let (status, json, _) = send(
        &fx.router,
        Method::PUT,
        &avatar_path,
        Some(&format!("Bearer {}", fx.token_a)),
        &[
            (KEY_HEADER, "k-missing-file"),
            (
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}").as_str(),
            ),
        ],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "avatar_invalid");

    // 畸形 multipart：缺 base_version 字段 → 400 avatar_invalid。
    body = multipart_body(BOUNDARY, "", "avatar.png", "image/png", PNG_BYTES);
    let (status, json, _) = send(
        &fx.router,
        Method::PUT,
        &avatar_path,
        Some(&format!("Bearer {}", fx.token_a)),
        &[
            (KEY_HEADER, "k-missing-base"),
            (
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}").as_str(),
            ),
        ],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "avatar_invalid");

    // base_version 非整数 → 400 avatar_invalid。
    body = multipart_body(BOUNDARY, "not-an-int", "avatar.png", "image/png", PNG_BYTES);
    let (status, json, _) = send(
        &fx.router,
        Method::PUT,
        &avatar_path,
        Some(&format!("Bearer {}", fx.token_a)),
        &[
            (KEY_HEADER, "k-bad-base"),
            (
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}").as_str(),
            ),
        ],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "avatar_invalid");
}

#[tokio::test]
async fn avatar_byte_read_round_trip_and_fail_closed() {
    let fx = fixture().await;
    let agent_id = create_agent(&fx.router, &fx.token_a, "助手", "a-1").await;
    let group = create_group(&fx.router, &fx.token_a, "群", "g-1").await;
    invite_member(&fx.router, &fx.token_a, &group, &agent_id, "inv-1").await;

    // 无头像 → 404 avatar_not_found。
    let resp = send_raw(
        &fx.router,
        Method::GET,
        &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "avatar_not_found");

    // PUT 头像 → GET 字节 round-trip。
    let put = put_avatar(
        &fx.router,
        &fx.token_a,
        &agent_id,
        "av-1",
        0,
        PNG_BYTES,
        "image/png",
    )
    .await;
    assert_eq!(put.status, StatusCode::CREATED, "{}", put.json());
    let resp = send_raw(
        &fx.router,
        Method::GET,
        &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(
        resp.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&resp.body)
    );
    assert_eq!(resp.body, PNG_BYTES, "bytes must round-trip");
    assert_eq!(
        resp.headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        resp.headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert!(
        resp.headers.contains_key("etag"),
        "etag must address the avatar version"
    );
    // 无路径/hash 泄漏。
    let body_text = String::from_utf8_lossy(&resp.body);
    assert!(
        !body_text.contains("storage") && !body_text.contains("sha256"),
        "bytes are raw, not json"
    );
    // 成员详情投影 avatar 存在性。
    let (status, json, _) = send(
        &fx.router,
        Method::GET,
        &format!("/v1/rooms/{group}/members/{agent_id}/profile"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["avatar"]["content_type"], "image/png");
    assert_eq!(
        json["profile"]["avatar"]["size_bytes"],
        PNG_BYTES.len() as i64
    );
    // member-presentations 摘要投影 avatar 存在性。
    let (status, json, _) = send(
        &fx.router,
        Method::GET,
        &format!("/v1/rooms/{group}/member-presentations"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let members = json["members"].as_array().unwrap();
    let m = members
        .iter()
        .find(|m| m["participant"]["id"] == agent_id)
        .expect("agent in summary");
    assert_eq!(m["avatar"]["content_type"], "image/png");
    let owner_m = members
        .iter()
        .find(|m| m["participant"]["kind"] == "human")
        .expect("human in summary");
    assert!(owner_m["avatar"].is_null(), "{owner_m:?}");

    // Bob 不是该群成员 → 404 room_not_found（不泄露）。
    let resp = send_raw(
        &fx.router,
        Method::GET,
        &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
        Some(&format!("Bearer {}", fx.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
    assert_eq!(resp.json()["error"]["code"], "room_not_found");

    // 摘除后 → 最新投影无头像、字节端点 404 avatar_not_found。
    let resp = delete_avatar(&fx.router, &fx.token_a, &agent_id, "del-1", 1).await;
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 2);
    assert!(resp.json()["profile"]["avatar"].is_null());
    let resp = send_raw(
        &fx.router,
        Method::GET,
        &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "avatar_not_found");
}

async fn delete_avatar(
    router: &axum::Router,
    token: &str,
    agent_id: &str,
    key: &str,
    base_version: i64,
) -> RawResponse {
    let body = format!(r#"{{"base_version":{base_version}}}"#);
    send_raw(
        router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/profile/avatar"),
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await
}

#[tokio::test]
async fn avatar_delete_lifecycle() {
    let fx = fixture().await;
    let agent_id = create_agent(&fx.router, &fx.token_a, "助手", "a-1").await;
    let avatar_path = format!("/v1/agents/{agent_id}/profile/avatar");

    // 无头像时 DELETE（base=0）→ 200 版本 1 avatar null（版本链推进，幂等语义一致）。
    let resp = delete_avatar(&fx.router, &fx.token_a, &agent_id, "del-0", 0).await;
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert!(resp.json()["profile"]["avatar"].is_null());

    // 同 key 同摘要重放 → 200 版本 1 不变、idempotency_replayed=true。
    let resp = delete_avatar(&fx.router, &fx.token_a, &agent_id, "del-0", 0).await;
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(resp.json()["idempotency_replayed"], true);

    // 同 key 不同请求（base 变化）→ 409 idempotency_conflict。
    let resp = delete_avatar(&fx.router, &fx.token_a, &agent_id, "del-0", 1).await;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");

    // 新 key 过期 base → 409 profile_stale。
    let resp = delete_avatar(&fx.router, &fx.token_a, &agent_id, "del-2", 0).await;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "profile_stale");

    // 非 owner → 404 agent_not_found。
    let resp = delete_avatar(&fx.router, &fx.token_b, &agent_id, "del-bob", 0).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "agent_not_found");

    // 未知字段 → 400 invalid_request。
    let (status, json, _) = send(
        &fx.router,
        Method::DELETE,
        &avatar_path,
        Some(&format!("Bearer {}", fx.token_a)),
        &[
            (KEY_HEADER, "del-bad"),
            ("content-type", "application/json"),
        ],
        Some(br#"{"base_version":0,"extra":1}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");
}
// ---------------------------------------------------------------------------
// 表达权第二刀（批次四）：Agent 写**自己**的头像
// ---------------------------------------------------------------------------

/// 给一支 Agent 发一份 Backend 凭据（走公开管理面：binding → credential）。
async fn agent_credential(
    router: &axum::Router,
    owner_token: &str,
    agent_id: &str,
    key: &str,
) -> String {
    let body = serde_json::json!({"label": key}).to_string().into_bytes();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(&format!("Bearer {owner_token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "binding: {json}");
    let binding = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding}/credentials"),
        Some(&format!("Bearer {owner_token}")),
        &[
            (KEY_HEADER, &format!("{key}-c")),
            ("content-type", "application/json"),
        ],
        Some(br#"{"replaces_credential_id":null}"#.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "credential: {json}");
    json["token"].as_str().unwrap().to_owned()
}

/// **一支 Agent 写自己的头像**（表达权第二刀）：在此之前它只能由 Owner 代笔。
#[tokio::test]
async fn an_agent_writes_its_own_avatar_via_backend_auth() {
    let fx = fixture().await;
    let agent_id = create_agent(&fx.router, &fx.token_a, "小满", "exp-av").await;
    let credential = agent_credential(&fx.router, &fx.token_a, &agent_id, "exp-av-b").await;
    let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];

    // 它自己写：`PUT /v1/backend/me/avatar`（身份只来自认证）。
    let body = multipart_body(BOUNDARY, "0", "avatar.png", "image/png", &png);
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    let response = send_raw(
        &fx.router,
        Method::PUT,
        "/v1/backend/me/avatar",
        Some(&format!("Bearer {credential}")),
        &[
            (KEY_HEADER, "exp-av-1"),
            ("content-type", content_type.as_str()),
        ],
        Some(body),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::CREATED,
        "它应当能写自己的头像: {}",
        response.json()
    );
    assert_eq!(response.json()["profile"]["profile_version"], 1);

    // 世界侧核对：Owner 那条既有读路径读得到它自己写的那一版。
    let (status, json, _) = send(
        &fx.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", fx.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["profile_version"], 1);
    assert_eq!(json["profile"]["published_by_kind"], "agent");

    // 摘除也是它自己：`DELETE /v1/backend/me/avatar`。
    let body = serde_json::json!({"base_version": 1})
        .to_string()
        .into_bytes();
    let (status, json, _) = send(
        &fx.router,
        Method::DELETE,
        "/v1/backend/me/avatar",
        Some(&format!("Bearer {credential}")),
        &[
            (KEY_HEADER, "exp-av-2"),
            ("content-type", "application/json"),
        ],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["profile_version"], 2);
}

/// 表达面与读取面同一条纪律：必须是 BackendAuth。
#[tokio::test]
async fn backend_avatar_write_requires_backend_auth() {
    let fx = fixture().await;
    let agent_id = create_agent(&fx.router, &fx.token_a, "助手", "exp-av2").await;
    let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
    let body = multipart_body(BOUNDARY, "0", "avatar.png", "image/png", &png);
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    let response = send_raw(
        &fx.router,
        Method::PUT,
        "/v1/backend/me/avatar",
        Some(&format!("Bearer {}", fx.token_a)),
        &[
            (KEY_HEADER, "exp-av-x"),
            ("content-type", content_type.as_str()),
        ],
        Some(body),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.json()
    );
    assert_eq!(response.json()["error"]["code"], "invalid_credential");
    let _ = agent_id;
}
