//! Backend SSE 唤醒流契约测试（窄解冻切片 CORE_V1_AGENT_PROTOCOL.md §8）：
//! 认证与四端点同语义（缺 header / Human token / 未知 token 统一 401）、
//! 初始唤醒与 post-commit 定向唤醒（真实 Message 提交 → agent_events 触发
//! → `events.available`）、流上无任何事实（token/room/正文绝不出现）、
//! Credential 撤销后唤醒前 re-auth fail-closed 断流、每 Agent 并发流上限。

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::header;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tokio_stream::StreamExt as _;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::identity;

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    #[allow(dead_code)]
    pool: SqlitePool,
    human_token: String,
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
    let human = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap alice");
    let router = build_router(
        Arc::new(store),
        Arc::new(identity),
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
        pool,
        human_token: human.token.as_str().to_owned(),
    }
}

async fn send(
    router: &axum::Router,
    method: Method,
    path: &str,
    authorization: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> (StatusCode, serde_json::Value) {
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
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json)
}

async fn create_agent(router: &axum::Router, token: &str, key: &str, name: &str) -> String {
    let (status, json) = send(
        router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {token}")),
        &[
            ("content-type", "application/json"),
            ("idempotency-key", key),
        ],
        Some(
            serde_json::json!({"display_name": name})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

async fn create_room(router: &axum::Router, token: &str, agent_id: &str, key: &str) -> String {
    let (status, json) = send(
        router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {token}")),
        &[
            ("content-type", "application/json"),
            ("idempotency-key", key),
        ],
        Some(
            serde_json::json!({"agent_id": agent_id})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    json["room"]["id"].as_str().unwrap().to_owned()
}

async fn send_human_message(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    body: &str,
    key: &str,
) -> StatusCode {
    let (status, json) = send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token}")),
        &[
            ("content-type", "application/json"),
            ("idempotency-key", key),
        ],
        Some(serde_json::json!({"body": body}).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    status
}

struct Ready {
    agent_id: String,
    binding_id: String,
    credential_id: String,
    token: String,
    room_id: String,
}

/// 标准场景：Human + Agent + Direct Room + Binding + Credential。
async fn ready(f: &Fixture, name: &str) -> Ready {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let agent_id = create_agent(&f.router, &f.human_token, &format!("a-{n}"), name).await;
    let room_id = create_room(&f.router, &f.human_token, &agent_id, &format!("r-{n}")).await;
    let (status, json) = send(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(&format!("Bearer {}", f.human_token)),
        &[
            ("content-type", "application/json"),
            ("idempotency-key", &format!("b-{n}")),
        ],
        Some(
            serde_json::json!({"label": "Runtime"})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    let binding_id = json["binding"]["id"].as_str().unwrap().to_owned();
    let (status, json) = send(
        &f.router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        Some(&format!("Bearer {}", f.human_token)),
        &[
            ("content-type", "application/json"),
            ("idempotency-key", &format!("c-{n}")),
        ],
        Some(
            serde_json::json!({"replaces_credential_id": null})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    Ready {
        agent_id,
        binding_id,
        credential_id: json["credential"]["id"].as_str().unwrap().to_owned(),
        token: json["token"].as_str().unwrap().to_owned(),
        room_id,
    }
}

type SseStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<Bytes, axum::Error>> + Send>>;

async fn open_stream(
    router: &axum::Router,
    authorization: Option<&str>,
) -> (StatusCode, HeaderMap, SseStream) {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri("/v1/backend/events/stream");
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let stream = response.into_body().into_data_stream();
    (status, headers, Box::pin(stream))
}

async fn next_frame(stream: &mut SseStream) -> Option<Bytes> {
    match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
        Ok(Some(Ok(bytes))) => Some(bytes),
        _ => None,
    }
}

async fn stream_ends_within(stream: &mut SseStream) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), stream.next()).await,
        Ok(None) | Ok(Some(Err(_)))
    )
}

fn assert_no_facts(frame: &Bytes, secrets: &[&str]) {
    let text = String::from_utf8_lossy(frame);
    for secret in secrets {
        assert!(
            !text.contains(secret),
            "doorbell frame must not carry facts; leaked {secret:?} in {text:?}"
        );
    }
}

/// 认证语义与四端点一致：缺 header `authentication_required`；Human
/// token / 未知 token 统一 `invalid_credential`（同一文案，无存在性
/// oracle）。
#[tokio::test]
async fn stream_requires_backend_credential() {
    let f = fixture().await;
    let ready = ready(&f, "Agent").await;

    let (status, json) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events/stream",
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        json["error"]["code"].as_str().unwrap(),
        "authentication_required"
    );

    for bad in [
        format!("Bearer {}", f.human_token),
        "Bearer clr_bc_unknown_token_value_0000000000000".to_owned(),
        "Bearer ".to_owned(),
    ] {
        let (status, json) = send(
            &f.router,
            Method::GET,
            "/v1/backend/events/stream",
            Some(&bad),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {json}");
        assert_eq!(
            json["error"]["code"].as_str().unwrap(),
            "invalid_credential"
        );
    }

    // 有效凭据可以建立流（拿到初始唤醒后即结束）。
    let (status, headers, mut stream) =
        open_stream(&f.router, Some(&format!("Bearer {}", ready.token))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
    );
    let frame = next_frame(&mut stream).await.expect("initial wake");
    assert!(String::from_utf8_lossy(&frame).contains("events.available"));
}

/// 建立即初始唤醒 + 真实 Message 提交后定向唤醒；流上无任何事实
/// （token / room id / 正文绝不出现）。
#[tokio::test]
async fn stream_yields_initial_and_post_commit_wake_without_facts() {
    let f = fixture().await;
    let ready = ready(&f, "Agent").await;

    let (status, _headers, mut stream) =
        open_stream(&f.router, Some(&format!("Bearer {}", ready.token))).await;
    assert_eq!(status, StatusCode::OK);

    let initial = next_frame(&mut stream).await.expect("initial wake frame");
    let initial_text = String::from_utf8_lossy(&initial).to_string();
    assert!(
        initial_text.contains("event: events.available"),
        "frame: {initial_text:?}"
    );
    assert_no_facts(&initial, &[&ready.token, &ready.room_id, "doorbell-probe"]);

    // Human 在 direct room 发消息 → 触发器生成 target Agent Event →
    // post-commit 定向唤醒 → 唤醒帧到达（REST 事实由 GET /v1/backend/events
    // 获取，此处只断言门铃）。
    send_human_message(
        &f.router,
        &f.human_token,
        &ready.room_id,
        "doorbell-probe",
        "m-1",
    )
    .await;

    let wake = next_frame(&mut stream)
        .await
        .expect("post-commit wake frame");
    let wake_text = String::from_utf8_lossy(&wake).to_string();
    assert!(
        wake_text.contains("events.available"),
        "frame: {wake_text:?}"
    );
    assert_no_facts(&wake, &[&ready.token, &ready.room_id, "doorbell-probe"]);
}

/// Credential 撤销后：下一次唤醒前 re-auth 失败 → 流立即结束（fail
/// closed；客户端重连将得到 401）。
#[tokio::test]
async fn stream_ends_after_credential_revocation() {
    let f = fixture().await;
    let ready = ready(&f, "Agent").await;

    let (status, _headers, mut stream) =
        open_stream(&f.router, Some(&format!("Bearer {}", ready.token))).await;
    assert_eq!(status, StatusCode::OK);
    let _ = next_frame(&mut stream).await.expect("initial wake");

    // 撤销 Credential（owner-scoped DELETE）。
    let (status, json) = send(
        &f.router,
        Method::DELETE,
        &format!(
            "/v1/agents/{}/backend-bindings/{}/credentials/{}",
            ready.agent_id, ready.binding_id, ready.credential_id
        ),
        Some(&format!("Bearer {}", f.human_token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body: {json}");

    // 新消息 → 新 Event → 唤醒 → re-auth 失败 → 流结束。
    send_human_message(
        &f.router,
        &f.human_token,
        &ready.room_id,
        "after revoke",
        "m-2",
    )
    .await;
    assert!(
        stream_ends_within(&mut stream).await,
        "stream must end after credential revocation (fail closed)"
    );

    // 重连得到 401（被踢下线语义）。
    let (status, json) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events/stream",
        Some(&format!("Bearer {}", ready.token)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {json}");
    assert_eq!(
        json["error"]["code"].as_str().unwrap(),
        "invalid_credential"
    );
}

/// 每 Agent 并发流 ≤2：第三条 503 backend_stream_limit_exceeded
/// （不排队、不降级）。
#[tokio::test]
async fn per_agent_stream_limit_is_enforced() {
    let f = fixture().await;
    let ready = ready(&f, "Agent").await;
    let auth = format!("Bearer {}", ready.token);

    let (status, _h, mut first) = open_stream(&f.router, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let _ = next_frame(&mut first).await.expect("first initial wake");
    let (status, _h, mut second) = open_stream(&f.router, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let _ = next_frame(&mut second).await.expect("second initial wake");

    let (status, json) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events/stream",
        Some(&auth),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {json}");
    assert_eq!(
        json["error"]["code"].as_str().unwrap(),
        "backend_stream_limit_exceeded"
    );

    // 释放第一条后额度可复用（转发任务在体被丢弃后的有界时间内回收）。
    drop(first);
    tokio::time::sleep(Duration::from_secs(6)).await;
    let (status, _h, _third) = open_stream(&f.router, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK, "released slot must be reusable");
}
