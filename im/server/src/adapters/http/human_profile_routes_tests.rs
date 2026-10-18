//! **人写自己的资料** HTTP 契约测试（表达权第四刀，2026-10-17 主人批准）。
//!
//! 判据（契约 `CORE_V1_PARTICIPANT_PRESENTATION.md` §4 与 `CORE_V1_AVATAR_P0.md`）：
//!
//! 1. **读自己**：`GET /v1/me/profile` 给出名字（身份事实）＋ 呈现（版本链那一格，可能为空）；
//! 2. **改名是状态写**：`PUT /v1/me/profile` 只动 `participants.display_name` 一列 ——
//!    没有版本行、没有幂等键；改完**既有端点 `GET /v1/me` 也变了**（同一份事实）；
//! 3. **不合法就 400 且状态不变**（空 / 全空白 / 含 NUL / 超 80 字符 / 未知字段）；
//! 4. **头像走版本链**：`PUT|DELETE /v1/me/profile/avatar`（multipart 单步、
//!    白名单 PNG/JPEG/WebP、≤2 MiB），版本严格 +1，摘除＝新版本 `avatar=null`；
//! 5. **改不到别人**：路径里没有"写谁的"（身份只来自认证）—— 两个 Human 各写各的；
//! 6. 没凭据一律 401。
//!
//! ⚠ fixture 与邻居同形（本仓库每个测试文件各自起一个真 SQLite + 真迁移）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
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

/// 1×1 的合法 PNG（头像字节卫生那一套走的是既有的白名单与摘要，不看内容语义）。
const PNG_1PX: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    pool: SqlitePool,
    token_a: String,
    human_a: String,
    token_b: String,
    human_b: String,
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
    let (human_b, token_b) = add_human(&pool, &identity, "Bob").await;
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
        pool,
        token_a: a.token.as_str().to_owned(),
        human_a: a.participant.id.as_str().to_owned(),
        token_b,
        human_b,
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

async fn get_profile(router: &axum::Router, token: &str) -> serde_json::Value {
    let (status, json, _) = send(
        router,
        Method::GET,
        "/v1/me/profile",
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    json
}

async fn rename(
    router: &axum::Router,
    token: &str,
    display_name: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "display_name": display_name }).to_string();
    let (status, json, _) = send(
        router,
        Method::PUT,
        "/v1/me/profile",
        Some(&format!("Bearer {token}")),
        &[("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    (status, json)
}

/// 头像写入的 multipart 体（单步：base_version + 文件字节一起走；key 走请求头）。
///
/// ⚠ 文件那一格**必须叫 `file`**：解析器的判据是"只许 base_version 与 file 两格"，
/// 名字错了就是 400 `avatar_invalid`（本测试第一版就踩了这个 —— 写成 `avatar` ✗）。
fn avatar_multipart(base_version: i64, content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----celira-human-profile-test";
    let mut body = Vec::new();
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    field("base_version", &base_version.to_string());
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n",
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn a_human_reads_their_own_profile_name_with_no_presentation_yet() {
    let f = fixture().await;
    let json = get_profile(&f.router, &f.token_a).await;
    assert_eq!(json["display_name"], "Alice");
    assert_eq!(
        json["profile"],
        serde_json::Value::Null,
        "从未写过 ⇒ null，不是空对象"
    );
    assert!(json["request_id"].is_string());
}

#[tokio::test]
async fn renaming_is_a_state_write_and_the_existing_me_endpoint_sees_it() {
    let f = fixture().await;
    let (status, json) = rename(&f.router, &f.token_a, "Alice 二号").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["display_name"], "Alice 二号", "响应是回读值");

    // **同一份事实**：既有端点 `GET /v1/me` 也变了（名字不是投影里的副本）。
    let (status, me, _) = send(
        &f.router,
        Method::GET,
        "/v1/me",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["participant"]["display_name"], "Alice 二号");

    // 另一个 Human 的名字**不受影响**（各写各的）。
    let other = get_profile(&f.router, &f.token_b).await;
    assert_eq!(other["display_name"], "Bob");

    // 状态写：**没有**版本行、**没有**幂等记录。
    let versions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM participant_profile_versions WHERE participant_id = ?",
    )
    .bind(&f.human_a)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        versions, 0,
        "改名不进版本链（display_name 是身份事实，不是呈现）"
    );
    let idempotency: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_put_idempotency")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(idempotency, 0, "状态写不落幂等记录（同值重写天然幂等）");

    // 同值重写天然幂等：再写一次同样的名字 ⇒ 还是 200、还是一个名字。
    let (status, again) = rename(&f.router, &f.token_a, "Alice 二号").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["display_name"], "Alice 二号");
}

#[tokio::test]
async fn an_invalid_name_is_rejected_and_changes_nothing() {
    let f = fixture().await;
    for bad in ["", "   ", "带\u{0}空字符", &"字".repeat(81)] {
        let (status, json) = rename(&f.router, &f.token_a, bad).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "名字 {bad:?} 应被拒：{json}"
        );
        assert_eq!(json["error"]["code"], "invalid_request");
        let after = get_profile(&f.router, &f.token_a).await;
        assert_eq!(after["display_name"], "Alice", "被拒的写不许改状态");
    }
}

#[tokio::test]
async fn an_unknown_field_is_rejected_instead_of_silently_ignored() {
    let f = fixture().await;
    let body = r#"{"display_name":"Alice 三号","nickname":"ali"}"#;
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        "/v1/me/profile",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json")],
        Some(body.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn writing_an_avatar_appends_a_profile_version_and_deleting_it_keeps_the_old_one() {
    let f = fixture().await;
    let (content_type, body) = avatar_multipart(0, "image/png", PNG_1PX);
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        "/v1/me/profile/avatar",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "k-avatar-1"), ("content-type", &content_type)],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    let after = get_profile(&f.router, &f.token_a).await;
    assert_eq!(after["profile"]["avatar"]["content_type"], "image/png");
    assert_eq!(
        after["profile"]["avatar"]["size_bytes"],
        PNG_1PX.len() as i64
    );
    assert_eq!(
        after["profile"]["published_by_id"], f.human_a,
        "署名是**我自己**（人没有 Owner 这一支）"
    );
    assert_eq!(after["profile"]["published_by_kind"], "human");
    let version = after["profile"]["profile_version"].as_i64().unwrap();
    assert_eq!(version, 1);

    // 摘除 = 新版本 avatar=null（旧版本行不可变保留 ⇒ 版本 +1、行数 +1）。
    let del = serde_json::json!({ "base_version": 1 }).to_string();
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        "/v1/me/profile/avatar",
        Some(&format!("Bearer {}", f.token_a)),
        &[
            (KEY_HEADER, "k-avatar-2"),
            ("content-type", "application/json"),
        ],
        Some(del.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let after = get_profile(&f.router, &f.token_a).await;
    assert_eq!(after["profile"]["avatar"], serde_json::Value::Null);
    assert_eq!(after["profile"]["profile_version"].as_i64().unwrap(), 2);
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM participant_profile_versions WHERE participant_id = ?",
    )
    .bind(&f.human_a)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(rows, 2, "两个版本行都在（旧版本不可变保留）");

    // **写自己碰不到别人**：另一个 Human 的版本链一格都没有
    // （身份只来自认证 ⇒ 路径里没有"写谁的"，从形状上就写不到别人）。
    let other_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM participant_profile_versions WHERE participant_id = ?",
    )
    .bind(&f.human_b)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(other_rows, 0, "另一个 Human 的呈现一个字都没动");
}

#[tokio::test]
async fn an_svg_avatar_is_rejected_by_the_whitelist() {
    let f = fixture().await;
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#;
    let (content_type, body) = avatar_multipart(0, "image/svg+xml", svg);
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        "/v1/me/profile/avatar",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "k-svg"), ("content-type", &content_type)],
        Some(body),
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "SVG 必须被拒（可执行类型），实际 {status}：{json}"
    );
}

#[tokio::test]
async fn both_endpoints_require_a_human_session() {
    let f = fixture().await;
    for (method, path, body) in [
        (Method::GET, "/v1/me/profile", None),
        (
            Method::PUT,
            "/v1/me/profile",
            Some(br#"{"display_name":"x"}"#.to_vec()),
        ),
    ] {
        let (status, json, _) = send(&f.router, method, path, None, &[], body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    }
}
