//! Agent Event HTTP 契约测试（CORE_V1_AGENT_EVENTS.md 8.x/9.x）：Backend
//! 认证通道隔离、严格 query 形状、分页与 next_after、无副作用重拉、
//! 多 Agent 隔离、Credential 轮换保持 sequence、无 Binding 间隙生成与
//! 重建后可观察、响应视图不含敏感字段、错误信封与 request id。

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
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::identity;
use crate::domain::backend::BackendToken;

const KEY_HEADER: &str = "idempotency-key";

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    pool: SqlitePool,
    token_a: String,
    human_a_id: String,
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
    let router = build_router(
        Arc::new(store),
        Arc::new(identity),
        Arc::new(agent),
        Arc::new(room),
        Arc::new(SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(SqliteEventStore::new(pool.clone())),
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
async fn create_agent(router: &axum::Router, token: &str, key: &str, name: &str) -> String {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        "/v1/agents",
        token,
        Some(key),
        Some(&serde_json::json!({"display_name": name})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

/// 通过公开 API 创建 Direct Room，返回 room_id。
async fn create_room(router: &axum::Router, token: &str, agent_id: &str, key: &str) -> String {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        "/v1/direct-rooms",
        token,
        Some(key),
        Some(&serde_json::json!({"agent_id": agent_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    json["room"]["id"].as_str().unwrap().to_owned()
}

/// 通过公开 API 发送一条 Message，返回 message id。
async fn send_message(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    body: &str,
    key: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        token,
        Some(key),
        Some(&serde_json::json!({"body": body})),
    )
    .await;
    (status, json)
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
    let (status, json, _) = request_json(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
        token,
        Some(key),
        Some(&serde_json::json!({"replaces_credential_id": replaces})),
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

async fn get_events(
    router: &axum::Router,
    token: &str,
    query: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let (status, json, request_id) = send(
        router,
        Method::GET,
        &format!("/v1/backend/events{query}"),
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    (status, json, request_id)
}

async fn event_count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn events_require_backend_auth_and_channel_isolation() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (c1, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    // Backend Credential 成功：空事件数组 200，next_after 为起点 sentinel。
    let (status, json, request_id) = get_events(&f.router, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["events"], serde_json::json!([]));
    assert_eq!(json["next_after"], "c1.be.start");
    assert!(request_id.is_some());
    assert_eq!(json["request_id"], request_id.as_deref().unwrap());

    // 缺 header → 401 authentication_required。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/events",
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");

    // Human Session token → 401 invalid_credential。
    let (status, json, _) = get_events(&f.router, &f.token_a, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_credential");

    // 格式错误 / 未知 token → 同一 401 invalid_credential。
    for bad in [
        "definitely-not-a-token",
        "clr_hs_0000000000000000000000000000000000000000000",
        &format!("clr_bc_{}", "a".repeat(42)),
    ] {
        let (status, json, _) = get_events(&f.router, bad, "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "bad token {bad:?}");
        assert_eq!(json["error"]["code"], "invalid_credential");
    }
    let unknown = BackendToken::generate().unwrap().as_str().to_owned();
    let (status, json, _) = get_events(&f.router, &unknown, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_credential");
    assert!(!json.to_string().contains("clr_bc_"));

    // 撤销 Credential 后立即 fail closed：先轮换出新 Credential 再撤销它。
    let (c2, token2) = issue_credential(
        &f.router,
        &f.token_a,
        &agent_id,
        &binding_id,
        Some(&c1),
        "c-2",
    )
    .await;
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials/{c2}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json, _) = get_events(&f.router, &token2, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_credential");
}

#[tokio::test]
async fn events_generated_without_binding_and_observable_after_binding_created() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;

    // 无 Binding 时发送：Message 201，数据库已生成 Event。
    let (status, message_json) =
        send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        event_count(&f).await,
        1,
        "event must exist without any binding"
    );

    // 幂等重放：200 且不新增 Event。
    let (status, json) = send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(event_count(&f).await, 1, "replay must not add an event");

    // 之后建立 Binding/Credential：从 after=0 能拉到这条间隙 Event。
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (_, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;
    let (status, json, _) = get_events(&f.router, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_ne!(
        events[0]["id"], message_json["message"]["id"],
        "event id must be distinct from the source message id"
    );
    assert_eq!(events[0]["message"]["id"], message_json["message"]["id"]);
    assert_eq!(events[0]["message"]["room_id"], room_id);
    assert_eq!(events[0]["message"]["body"], "hello");
    assert_eq!(events[0]["kind"], "message.created");
    assert_eq!(events[0]["created_at"], events[0]["message"]["created_at"]);
    assert!(
        events[0].get("sequence").is_none(),
        "no sequence in event view"
    );
    assert!(
        events[0]["message"].get("sequence").is_none(),
        "no sequence in source message view"
    );
    assert_eq!(
        json["next_after"],
        serde_json::json!(format!("c1.be.{}", events[0]["id"].as_str().unwrap()))
    );
}

#[tokio::test]
async fn event_response_excludes_target_binding_credential_owner_and_deferred_fields() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    send_message(&f.router, &f.token_a, &room_id, "private", "m-1").await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (_, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    let (status, json, _) = get_events(&f.router, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    let event = &json["events"][0];
    let body = event.to_string();
    for forbidden in [
        "target_agent_id",
        "target",
        "binding",
        "credential",
        "owner",
        "idempotency",
        "token",
        "hash",
        "cursor",
        "completion",
        "ack",
        "attempt",
        "lease",
    ] {
        assert!(
            !body.contains(&format!("\"{forbidden}\"")),
            "event view must not expose {forbidden}"
        );
    }
    // actor 是读取时当前 Human 公共资料投影。
    assert_eq!(event["message"]["actor"]["id"], f.human_a_id);
    assert_eq!(event["message"]["actor"]["kind"], "human");
    assert_eq!(event["message"]["actor"]["display_name"], "Alice");
    assert_eq!(event["message"]["actor"]["status"], "active");
    // Event ID 是 UUID v4。
    let id = event["id"].as_str().unwrap();
    let uuid = Uuid::parse_str(id).unwrap();
    assert_eq!(uuid.get_version(), Some(uuid::Version::Random));
}

#[tokio::test]
async fn strict_query_shapes_reject_unknown_duplicate_illegal_and_malformed_cursors() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (_, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    for query in [
        "?foo=1",
        "?after=1&foo=2",
        "?after=1&after=2",
        "?limit=1&limit=2",
        "?after=",
        "?after=-1",
        "?after=abc",
        "?after=9223372036854775808",
        "?after=c1.m.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.he.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.be.not-a-uuid",
        "?after=c1.be.550E8400-E29B-41D4-A716-446655440000",
        "?after=c2.be.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.be.550e8400-e29b-41d4-a716-446655440000",
        "?limit=0",
        "?limit=101",
        "?limit=abc",
        "?limit=-1",
        "?limit=4294967296",
        "?after=1&limit=2&limit=3",
        "?after=%",
        "?after=%GG",
        "?after=%FF",
        "?%FF=1",
    ] {
        let (status, json, _) = get_events(&f.router, &token, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "query {query}");
        assert_eq!(json["error"]["code"], "invalid_request", "query {query}");
    }
}

#[tokio::test]
async fn cursor_round_trip_pagination_empty_page_stability_and_default_start() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    for i in 0..5 {
        let (status, _) = send_message(
            &f.router,
            &f.token_a,
            &room_id,
            &format!("msg {i}"),
            &format!("m-{i}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (_, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    // 默认：limit=50 返回全部 5 条升序；next_after 为最后一条的 cursor。
    let (status, json, _) = get_events(&f.router, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<String> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 5);
    assert_eq!(
        json["next_after"],
        serde_json::json!(format!("c1.be.{}", ids[4]))
    );

    // 分页：limit=2 → [1,2]；回传 next_after cursor → [3,4]；再回传 → [5]。
    let (status, json, _) = get_events(&f.router, &token, "?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    let page1_ids: Vec<String> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page1_ids, ids[0..2]);
    let next1 = json["next_after"].as_str().unwrap().to_owned();
    assert_eq!(next1, format!("c1.be.{}", page1_ids[1]));

    let (status, json, _) = get_events(&f.router, &token, &format!("?after={next1}&limit=2")).await;
    assert_eq!(status, StatusCode::OK);
    let page2_ids: Vec<String> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page2_ids, ids[2..4]);
    let next2 = json["next_after"].as_str().unwrap().to_owned();

    let (status, json, _) = get_events(&f.router, &token, &format!("?after={next2}")).await;
    assert_eq!(status, StatusCode::OK);
    let page3_ids: Vec<String> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page3_ids, ids[4..]);
    assert_eq!(
        json["next_after"],
        serde_json::json!(format!("c1.be.{}", ids[4]))
    );

    // 空页：next_after 保持实际请求 cursor（原样回传）。
    let last_cursor = format!("c1.be.{}", ids[4]);
    let (status, json, _) = get_events(&f.router, &token, &format!("?after={last_cursor}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["events"], serde_json::json!([]));
    assert_eq!(json["next_after"], serde_json::json!(last_cursor.clone()));
    // 起点 sentinel：显式传入与缺省一致（从起点返回全部既有 Event）。
    let (status, json, _) = get_events(&f.router, &token, "?after=c1.be.start").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["events"].as_array().unwrap().len(),
        5,
        "start sentinel behaves like the omitted after"
    );
    assert_eq!(json["next_after"], serde_json::json!(last_cursor.clone()));

    // 同一 cursor 重拉：返回相同既有 Event，总数不变（读取无副作用）。
    let (status, first_json, _) = get_events(&f.router, &token, "?limit=2").await;
    let (status2, second_json, _) = get_events(&f.router, &token, "?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(
        first_json["events"], second_json["events"],
        "re-pull must return the same events"
    );
    assert_eq!(first_json["next_after"], second_json["next_after"]);
    assert_eq!(event_count(&f).await, 5);
    let bindings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_bindings")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(bindings, 1, "observing must not create or alter bindings");
}

#[tokio::test]
async fn two_agents_are_isolated_and_cursors_do_not_cross() {
    let f = fixture().await;
    let agent_a = create_agent(&f.router, &f.token_a, "a-1", "甲").await;
    let agent_b = create_agent(&f.router, &f.token_a, "a-2", "乙").await;
    let room_a = create_room(&f.router, &f.token_a, &agent_a, "r-1").await;
    let room_b = create_room(&f.router, &f.token_a, &agent_b, "r-2").await;
    for i in 0..3 {
        let (status, _) = send_message(
            &f.router,
            &f.token_a,
            &room_a,
            &format!("a {i}"),
            &format!("a-{i}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    send_message(&f.router, &f.token_a, &room_b, "b only", "b-1").await;
    let binding_a = create_binding(&f.router, &f.token_a, &agent_a, "A", "b-a").await;
    let binding_b = create_binding(&f.router, &f.token_a, &agent_b, "B", "b-b").await;
    let (_, token_a) =
        issue_credential(&f.router, &f.token_a, &agent_a, &binding_a, None, "c-a").await;
    let (_, token_b) =
        issue_credential(&f.router, &f.token_a, &agent_b, &binding_b, None, "c-b").await;

    let (status, json, _) = get_events(&f.router, &token_a, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["events"].as_array().unwrap().len(),
        3,
        "agent A sees only its own 3"
    );
    let (status, json, _) = get_events(&f.router, &token_b, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["events"].as_array().unwrap().len(),
        1,
        "agent B sees only its own 1"
    );

    // 跨 Agent cursor：把 A 的事件 cursor 用到 B 的端点 → 400 invalid_request，
    // 与未知 cursor 响应完全一致（不泄露存在性或归属）。
    let a_cursor = {
        let (_, json, _) = get_events(&f.router, &token_a, "?limit=1").await;
        json["next_after"].as_str().unwrap().to_owned()
    };
    let (status, json, _) = get_events(&f.router, &token_b, &format!("?after={a_cursor}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "cross-agent cursor");
    assert_eq!(json["error"]["code"], "invalid_request");
    let unknown = format!("c1.be.{}", Uuid::new_v4());
    let (status, json, _) = get_events(&f.router, &token_b, &format!("?after={unknown}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    // 无跨 Agent 泄露：B 的响应不含 A 的 room。
    let body = json.to_string();
    assert!(!body.contains(&format!("\"room_id\":\"{room_a}\"")));
}

#[tokio::test]
async fn credential_rotation_keeps_the_same_event_sequence() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
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

    // 旧 token 401；新 token 从同一 Agent 继续观察，Event 不变。
    let (status, _, _) = get_events(&f.router, &token1, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, json, _) = get_events(&f.router, &token2, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["events"][0]["message"]["body"], "hello");
    assert_eq!(
        json["next_after"],
        serde_json::json!(format!(
            "c1.be.{}",
            json["events"][0]["id"].as_str().unwrap()
        ))
    );
}

#[tokio::test]
async fn binding_revoke_keeps_events_and_rebuild_observes_gap_events() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    send_message(&f.router, &f.token_a, &room_id, "gap", "m-1").await;
    let binding_1 = create_binding(&f.router, &f.token_a, &agent_id, "one", "b-1").await;
    let (_, token_1) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_1, None, "c-1").await;

    // 撤销 Binding → 旧 Credential 401；Event 行保留。
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_1}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = get_events(&f.router, &token_1, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        event_count(&f).await,
        1,
        "binding revoke must keep event rows"
    );

    // 无 Binding 间隙中继续产生 Event，重建 Binding/Credential 后可观察。
    send_message(&f.router, &f.token_a, &room_id, "gap 2", "m-2").await;
    assert_eq!(event_count(&f).await, 2);
    let binding_2 = create_binding(&f.router, &f.token_a, &agent_id, "two", "b-2").await;
    let (_, token_2) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_2, None, "c-2").await;
    let (status, json, _) = get_events(&f.router, &token_2, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["events"].as_array().unwrap().len(),
        2,
        "rebuilt backend observes the gap events"
    );
    assert_eq!(json["events"][0]["message"]["body"], "gap");
    assert_eq!(json["events"][1]["message"]["body"], "gap 2");
}

#[tokio::test]
async fn agent_revoke_fails_closed_and_keeps_event_rows() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    send_message(&f.router, &f.token_a, &room_id, "before revoke", "m-1").await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (_, token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;
    let (status, json, _) = get_events(&f.router, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["events"][0]["message"]["body"], "before revoke");

    // Agent revoke → Credential 401；Event 历史行保留；Human History 不变。
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
    let (status, json, _) = get_events(&f.router, &token, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_credential");
    assert_eq!(
        event_count(&f).await,
        1,
        "agent revoke must keep event rows"
    );
    let (status, json, _) = request_json(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/messages"),
        &f.token_a,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn errors_never_leak_credential_or_event_facts() {
    let f = fixture().await;
    let agent_id = create_agent(&f.router, &f.token_a, "a-1", "助手").await;
    let room_id = create_room(&f.router, &f.token_a, &agent_id, "r-1").await;
    send_message(&f.router, &f.token_a, &room_id, "hello", "m-1").await;
    let binding_id = create_binding(&f.router, &f.token_a, &agent_id, "local runtime", "b-1").await;
    let (c0, _token) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-1").await;

    // 撤销后 401 错误体不得包含 token 前缀或 hash。
    let (c1, _token1) = issue_credential(
        &f.router,
        &f.token_a,
        &agent_id,
        &binding_id,
        Some(&c0),
        "c-2",
    )
    .await;
    let (c2, token2) = issue_credential(
        &f.router,
        &f.token_a,
        &agent_id,
        &binding_id,
        Some(&c1),
        "c-3",
    )
    .await;
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials/{c2}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json, _) = get_events(&f.router, &token2, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let body = json.to_string();
    assert!(
        !body.contains("clr_bc_"),
        "error body must not contain token prefix"
    );
    assert!(!body.contains("token_hash"));
    // 400 错误体同样不含 token（用当前 active Credential 验证）。
    let (_c3, token3) =
        issue_credential(&f.router, &f.token_a, &agent_id, &binding_id, None, "c-4").await;
    let (status, json, _) = get_events(&f.router, &token3, "?after=abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!json.to_string().contains("clr_bc_"));
}
