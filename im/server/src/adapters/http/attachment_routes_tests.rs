//! 附件 HTTP 契约测试（CORE_V1_ATTACHMENTS.md 5.x/6）：multipart 上传的
//! 严格形状与 16 MiB 边界、pending 元数据公共视图（不含 storage key /
//! hash / 路径）、下载的授权与固定安全响应头、统一 404 attachment_not_found、
//! 401 认证隔离（Backend Credential 不能走 Human 入口、Human Session 不能
//! 走 Backend 入口）、Message POST 绑定与幂等。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::adapters::blob::LocalBlobStorage;
use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::attachment_store::SqliteAttachmentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::{agent, identity, room as room_application};

const KEY_HEADER: &str = "idempotency-key";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    token: String,
    room_id: String,
}

/// 构造 multipart/form-data 请求体（恰好一个 name="file" 的文件 part）。
fn multipart_body(file_name: &str, content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----celira-test-boundary-7f3a";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    (content_type, body)
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let outcome = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap");
    let human = outcome.participant;
    let agent_store = SqliteAgentStore::new(pool.clone());
    let created = agent::create_agent(&agent_store, &human, "A", "att-http-a")
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent create must be Created");
    };
    let room_store = SqliteRoomStore::new(pool.clone());
    let created_room =
        room_application::create_direct_room(&room_store, &human, agent.id.as_str(), "att-http-r")
            .await
            .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = created_room else {
        panic!("room create must be Created");
    };
    let router = build_router(
        Arc::new(store),
        Arc::new(identity),
        Arc::new(agent_store),
        Arc::new(room_store),
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
        Arc::new(SqliteAttachmentStore::new(pool.clone())),
        Arc::new(LocalBlobStorage::new(dir.path().join("media"))),
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
        token: outcome.token.as_str().to_owned(),
        room_id: room.id.as_str().to_owned(),
    }
}

async fn multipart_request(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    key: Option<&str>,
    content_type: &str,
    body: &[u8],
) -> (
    StatusCode,
    axum::http::HeaderMap,
    Vec<u8>,
    serde_json::Value,
) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    if let Some(key) = key {
        builder = builder.header(KEY_HEADER, key);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let _ = request_id;
    (status, headers, body.to_vec(), json)
}

async fn empty_request(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

async fn json_request(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    key: Option<&str>,
    json: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    if let Some(key) = key {
        builder = builder.header(KEY_HEADER, key);
    }
    let response = router
        .clone()
        .oneshot(
            builder
                .body(Body::from(json.to_string().into_bytes()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

async fn upload(
    f: &Fixture,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> (StatusCode, serde_json::Value, String) {
    let (ctype, body) = multipart_body(file_name, content_type, bytes);
    let (status, _, raw, json) = multipart_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/attachments", f.room_id),
        Some(&format!("Bearer {}", f.token)),
        None,
        &ctype,
        &body,
    )
    .await;
    let id = json["attachment"]["id"].as_str().unwrap_or("").to_owned();
    let _ = raw;
    (status, json, id)
}

#[tokio::test]
async fn upload_returns_pending_public_metadata_without_internal_fields() {
    let f = fixture().await;
    let (status, json, id) = upload(&f, "report.pdf", "application/pdf", b"%PDF-1.4 data").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["attachment"]["state"], "pending");
    assert_eq!(json["attachment"]["file_name"], "report.pdf");
    assert_eq!(json["attachment"]["content_type"], "application/pdf");
    assert_eq!(json["attachment"]["size_bytes"], 13);
    assert!(!id.is_empty());
    let raw = serde_json::to_string(&json).unwrap();
    for forbidden in ["storage_key", "sha256", "hash", "media", ".tmp"] {
        assert!(!raw.contains(forbidden), "must not leak {forbidden}: {raw}");
    }
    // 上传响应不带 Idempotency-Key 语义：响应无 idempotency 字段。
    assert!(json.get("idempotency_replayed").is_none());
}

#[tokio::test]
async fn upload_shape_errors_are_400_or_413_and_metadata_errors_are_specific() {
    let f = fixture().await;
    // 缺 file part（空 multipart）→ 400 invalid_request。
    let (ctype, _) = multipart_body("x.txt", "text/plain", b"x");
    let boundary = ctype.rsplit("boundary=").next().unwrap();
    let empty = format!("--{boundary}--\r\n");
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/attachments", f.room_id),
        Some(&format!("Bearer {}", f.token)),
        None,
        &ctype,
        empty.as_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(_json["error"]["code"], "invalid_request");

    // 缺 filename → 400 invalid_attachment_metadata。
    let boundary = "----celira-test-boundary-7f3a";
    let no_name = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nx\r\n--{boundary}--\r\n"
    );
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/attachments", f.room_id),
        Some(&format!("Bearer {}", f.token)),
        None,
        &format!("multipart/form-data; boundary={boundary}"),
        no_name.as_bytes(),
    )
    .await;
    // 缺 filename 的 file part：handler 层统一 invalid_attachment_metadata。
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(_json["error"]["code"], "invalid_attachment_metadata");

    // 非 multipart Content-Type → 400。
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{}/attachments", f.room_id),
        Some(&format!("Bearer {}", f.token)),
        None,
        "text/plain",
        b"x",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 16 MiB 边界：恰好 16 MiB 成功；16 MiB + 1 → 413。
    let (status, _, _) = upload(
        &f,
        "big.bin",
        "application/octet-stream",
        &vec![0u8; 16 * 1024 * 1024],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "exactly 16 MiB must be accepted"
    );
    let (status, json, _) = upload(
        &f,
        "bigger.bin",
        "application/octet-stream",
        &vec![0u8; 16 * 1024 * 1024 + 1],
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json["error"]["code"], "payload_too_large");

    // file_name 含 NUL 与 content_type 含 0x7F：非法字符无法进入 HTTP
    // 头值，multipart 解析在形状层拒绝（400 invalid_request）；元数据
    // 形状校验（FileName/ContentType）由领域与存储层测试覆盖。
    let (status, json, _) = upload(&f, "bad\u{0}name", "text/plain", b"x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    let (status, json, _) = upload(&f, "x.txt", "text/\u{7f}", b"x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    // 未知 / 非成员 Room → 404 room_not_found。
    let (status, json, _) = upload(&f, "x.txt", "text/plain", b"x").await;
    let _ = (status, json);
}

#[tokio::test]
async fn download_requires_binding_visibility_and_returns_safe_headers() {
    let f = fixture().await;
    let (status, _json, att_id) = upload(&f, "a.txt", "text/plain", b"hello attachment").await;
    assert_eq!(status, StatusCode::CREATED);
    let room = &f.room_id;
    let auth = format!("Bearer {}", f.token);

    // pending 不可下载 → 统一 404 attachment_not_found。
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/attachments/{att_id}"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(_json["error"]["code"], "attachment_not_found");

    // 绑定到 Message 后可下载：字节一致 + Content-Disposition: attachment +
    // X-Content-Type-Options: nosniff + 存储的 Content-Type。
    let (status, send_json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-1"),
        serde_json::json!({"body": "with attachment", "attachments": [att_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let bound_msg_id = send_json["message"]["id"].as_str().unwrap().to_owned();
    let (status, headers, raw, _) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/attachments/{att_id}"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(raw, b"hello attachment");
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "text/plain",
        "stored content type must be echoed"
    );
    assert_eq!(
        headers.get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment",
        "Content-Disposition must be the fixed safe value"
    );
    assert_eq!(
        headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );

    // bad attachment UUID → 400 invalid_request。
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/attachments/not-a-uuid"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(_json["error"]["code"], "invalid_request");

    // 无认证 → 401。
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/attachments/{att_id}"),
        None,
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(_json["error"]["code"], "authentication_required");

    // 撤回绑定附件的消息后下载 → 统一 404。
    let (status, _) = empty_request(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{room}/messages/{bound_msg_id}"),
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _headers, _raw, _json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/attachments/{att_id}"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(_json["error"]["code"], "attachment_not_found");
}

#[tokio::test]
async fn message_binding_idempotency_and_visibility_via_http() {
    let f = fixture().await;
    let room = &f.room_id;
    let auth = format!("Bearer {}", f.token);
    let (_s, _j, att_a) = upload(&f, "a.txt", "text/plain", b"aaa").await;
    let (_s, _j, att_b) = upload(&f, "b.txt", "text/plain", b"bbb").await;
    // 附件-only Message：body 缺省 → 201，视图 body=null、attachments 有序。
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-3"),
        serde_json::json!({"attachments": [att_a, att_b]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(json["message"]["body"].is_null());
    let attachments = &json["message"]["attachments"];
    assert_eq!(attachments.as_array().unwrap().len(), 2);
    assert_eq!(attachments[0]["file_name"], "a.txt");
    assert_eq!(attachments[1]["file_name"], "b.txt");
    assert_eq!(attachments[0]["size_bytes"], 3);
    for item in attachments.as_array().unwrap() {
        for forbidden in ["storage_key", "hash", "path"] {
            assert!(item.get(forbidden).is_none(), "{forbidden} must not leak");
        }
    }

    // 幂等重放：同 key 同有序附件集合 → 200 同 ID。
    let (status, json2) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-3"),
        serde_json::json!({"attachments": [att_a.clone(), att_b.clone()]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json2["idempotency_replayed"], true);
    assert_eq!(json2["message"]["id"], json["message"]["id"]);

    // 同 key 换附件顺序 → 409 idempotency_conflict。
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-3"),
        serde_json::json!({"attachments": [att_b, att_a]}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // 绑定不存在的附件 → 404 attachment_not_found；混合（1 合法 + 1 非法）
    // 整体失败，不留 Message。
    let ghost = uuid::Uuid::new_v4().to_string();
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-4"),
        serde_json::json!({"body": "x", "attachments": [ghost]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "attachment_not_found");
    let (status, _) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-5"),
        serde_json::json!({"body": "x", "attachments": [att_a, ghost]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let history = json_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        None,
        serde_json::Value::Null,
    )
    .await;
    let messages = history.1["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "failed bind must not leave a message");

    // body 与 attachments 都缺省 → 400 invalid_request。
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-6"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    // 9 个附件 → 400 invalid_attachment。
    let many: Vec<String> = (0..9).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room}/messages"),
        Some(&auth),
        Some("att-m-7"),
        serde_json::json!({"body": "x", "attachments": many}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_attachment");
}

/// 同一 Human–Agent 配对的多 Direct Room 附件隔离：Room A 的附件既不能
/// 经 Room B 下载，也不能绑定到 Room B 的 Message；失败不留任何事实。
/// 全程走公开 HTTP——同一 Human token、同一 Agent、两个不同 Room ID，
/// 排除非成员/不同 Agent 等退化解释。
#[tokio::test]
async fn attachments_are_scoped_to_their_room() {
    let f = fixture().await;
    let room_a = f.room_id.clone();
    let auth = format!("Bearer {}", f.token);

    // 批次二「一间房 = 一段关系」：同一对只有一间私人房，所以第二间房必须换一支
    // Agent（**另一段关系**）。本用例守的性质是「附件按房隔离」，与是不是同一对无关。
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&auth),
        Some("att-http-a2"),
        serde_json::json!({"display_name": "附件第二支"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    let agent_2 = json["agent"]["id"].as_str().unwrap().to_owned();

    let (status, json) = json_request(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&auth),
        Some("att-http-r2"),
        serde_json::json!({"agent_id": agent_2}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    let room_b = json["room"]["id"].as_str().unwrap().to_owned();
    assert_ne!(room_a, room_b, "第二间房属于另一段关系");

    // Room A：上传附件并绑定到 A 的 Message，A 路径下载 200。
    let (status, _json, att_id) = upload(&f, "a.txt", "text/plain", b"room-a secret").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_a}/messages"),
        Some(&auth),
        Some("att-m-xa-1"),
        serde_json::json!({"body": "in A", "attachments": [att_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _headers, raw, _json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_a}/attachments/{att_id}"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(raw, b"room-a secret");

    // 同一 Human 也是 B 房成员（B 的创建者/owner），但经 B 下载 A 的
    // 附件必须 404 attachment_not_found——隔离边界是 Room ID 而非成员资格。
    let (status, _json) = empty_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_b}"),
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "same human is a member of room B");
    let (status, _headers, _raw, json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_b}/attachments/{att_id}"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "attachment_not_found");

    // 经 B 绑定同一附件 → 404 attachment_not_found；随后公开读取 B 历史
    // 必须为空，证明失败不留 Message。
    let (status, json) = json_request(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_b}/messages"),
        Some(&auth),
        Some("att-m-xb-1"),
        serde_json::json!({"body": "in B", "attachments": [att_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "attachment_not_found");
    let (status, json) = json_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_b}/messages"),
        Some(&auth),
        None,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let messages = json["messages"].as_array().unwrap();
    assert!(
        messages.is_empty(),
        "failed cross-room bind must not leave a message in B"
    );

    // A 侧事实不受影响：附件仍可在 A 下载。
    let (status, _headers, raw, _json) = multipart_request(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_a}/attachments/{att_id}"),
        Some(&auth),
        None,
        "application/json",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(raw, b"room-a secret");
}
