//! Participant Presentation HTTP 契约测试
//! （CORE_V1_PARTICIPANT_PRESENTATION.md §3–§7）：owner-scoped Profile
//! GET/PUT、Room member presentation summary / member detail / member
//! presentation GET、Group owner Presentation PUT；幂等/并发/stale、授权失败
//! 路径、错误信封与既有 `/v1/rooms/{room_id}/members` 形状不变。

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

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    pool: SqlitePool,
    token_a: String,
    human_a: String,
    token_b: String,
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session（非成员 / 非 owner
/// 场景用；与既有 fixture 同一路径）。
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
        pool,
        token_a: a.token.as_str().to_owned(),
        human_a: a.participant.id.as_str().to_owned(),
        token_b,
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

async fn create_agent(
    router: &axum::Router,
    token: &str,
    display_name: &str,
    key: &str,
) -> serde_json::Value {
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
    json
}

async fn create_group(
    router: &axum::Router,
    token: &str,
    name: &str,
    key: &str,
) -> serde_json::Value {
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
    json
}

async fn invite_member(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    participant_id: &str,
    key: &str,
) -> (StatusCode, serde_json::Value) {
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
    (status, json)
}

/// 建立：Owner 的 Agent + 一个 group Room（owner=A）+ 把 agent 邀请进群。
async fn seeded_world(f: &Fixture) -> (String, String) {
    let agent = create_agent(&f.router, &f.token_a, "助手", "agent-key").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let group = create_group(&f.router, &f.token_a, "group", "group-key").await;
    let room_id = group["room"]["id"].as_str().unwrap().to_owned();
    let (status, _) = invite_member(&f.router, &f.token_a, &room_id, &agent_id, "invite-key").await;
    assert_eq!(status, StatusCode::CREATED);
    (agent_id, room_id)
}

fn put_profile_body(base_version: i64, short: Option<&str>, public: Option<&str>) -> String {
    let short = short
        .map(|s| format!(r#""{s}""#))
        .unwrap_or_else(|| "null".to_owned());
    let public = public
        .map(|s| format!(r#""{s}""#))
        .unwrap_or_else(|| "null".to_owned());
    format!(
        r#"{{"base_version":{base_version},"short_description":{short},"public_description":{public}}}"#
    )
}

// ============================================================
// Owner-scoped Agent Profile
// ============================================================

#[tokio::test]
async fn profile_get_returns_null_before_any_write() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap();
    let (status, json, request_id) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["agent"]["id"], agent_id);
    assert_eq!(json["ownership"]["human_id"], f.human_a);
    assert!(json["profile"].is_null());
    assert_eq!(json["request_id"], request_id.unwrap());
}

#[tokio::test]
async fn profile_put_create_then_update_then_replay_preserves_original_status() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/agents/{agent_id}/profile");
    let auth = format!("Bearer {}", f.token_a);

    // 首次 → 201，result_is_create（版本 0 → 1）。
    let (status, first, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[
            (KEY_HEADER, "profile-1"),
            ("content-type", "application/json"),
        ],
        Some(put_profile_body(0, Some("short"), Some("public")).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["profile"]["profile_version"], 1);
    assert_eq!(first["idempotency_replayed"], false);
    assert_eq!(first["profile"]["published_by_kind"], "human");
    assert_eq!(first["profile"]["short_description"], "short");
    assert_eq!(first["profile"]["public_description"], "public");

    // 更新 → 200（追加版本 2）。
    let (status, second, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[
            (KEY_HEADER, "profile-2"),
            ("content-type", "application/json"),
        ],
        Some(put_profile_body(1, Some("short2"), Some("public2")).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["profile"]["profile_version"], 2);
    assert_eq!(second["idempotency_replayed"], false);

    // 同 key 同摘要重放首次 → 201（返回首次成功状态）+ replayed=true，
    // 即使 head 已推进到 2。
    let (status, replay, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[
            (KEY_HEADER, "profile-1"),
            ("content-type", "application/json"),
        ],
        Some(put_profile_body(0, Some("short"), Some("public")).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{replay}");
    assert_eq!(replay["profile"]["profile_version"], 1);
    assert_eq!(replay["idempotency_replayed"], true);

    let profile_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(profile_rows, 2, "replay must not append a version");
}

#[tokio::test]
async fn profile_put_same_key_different_payload_conflicts_and_no_side_effects() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/agents/{agent_id}/profile");
    let auth = format!("Bearer {}", f.token_a);

    let (status, _, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "prof-c"), ("content-type", "application/json")],
        Some(put_profile_body(0, Some("first"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, json, request_id) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "prof-c"), ("content-type", "application/json")],
        Some(put_profile_body(0, Some("changed"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    assert_eq!(json["error"]["request_id"], request_id.unwrap());

    // 新 key + stale base_version → profile_stale。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "prof-s"), ("content-type", "application/json")],
        Some(put_profile_body(0, Some("stale"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "profile_stale");

    let profile_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        profile_rows, 1,
        "conflicts and stale must not leave versions"
    );
}

#[tokio::test]
async fn profile_invalid_base_version_nul_and_byte_limits_are_400() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/agents/{agent_id}/profile");
    let auth = format!("Bearer {}", f.token_a);

    // 负 base_version → profile_invalid。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[
            (KEY_HEADER, "p-negative"),
            ("content-type", "application/json"),
        ],
        Some(put_profile_body(-1, Some("x"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "profile_invalid");

    // NUL → profile_invalid。
    let body = r#"{"base_version":0,"short_description":"a\u0000b","public_description":null}"#; // NUL escape stays verbatim in JSON
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "p-nul"), ("content-type", "application/json")],
        Some(body.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "profile_invalid");

    // 超短描述字节上限（513 ASCII）→ profile_invalid。
    let long = "a".repeat(513);
    let body =
        format!(r#"{{"base_version":0,"short_description":"{long}","public_description":null}}"#);
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "p-long"), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "profile_invalid");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn profile_unknown_bad_id_and_non_owner_are_uniform_404_revoked_is_409() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();

    // 未知 / bad-id → 404。
    for path in [
        "/v1/agents/00000000-0000-0000-0000-000000000000/profile".to_owned(),
        "/v1/agents/not-a-uuid/profile".to_owned(),
    ] {
        let (status, json, _) = send(
            &f.router,
            Method::GET,
            &path,
            Some(&format!("Bearer {}", f.token_a)),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {json}");
        assert_eq!(json["error"]["code"], "agent_not_found");
    }

    // 非 owner（Bob）读 Alice 的 agent → 404。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "agent_not_found");

    // 非 owner PUT → 404，无副作用。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_b)),
        &[(KEY_HEADER, "p-b"), ("content-type", "application/json")],
        Some(put_profile_body(0, Some("x"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "agent_not_found");

    // 撤销 Agent（owner 关系同时 revoked 且 agent 不可再写）→ GET 409。
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
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "agent_not_active");

    // 撤销后重放不成立：即使曾有 profile 写入，授权先于重放 → 409。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            (KEY_HEADER, "p-revoke-replay"),
            ("content-type", "application/json"),
        ],
        Some(put_profile_body(0, Some("x"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "agent_not_active");
}

#[tokio::test]
async fn profile_malformed_unknown_missing_fields_and_missing_key_are_400() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap();
    let path = format!("/v1/agents/{agent_id}/profile");
    let auth = format!("Bearer {}", f.token_a);

    let cases: Vec<Vec<u8>> = vec![
        b"not json".to_vec(),
        b"{}".to_vec(),
        r#"{"base_version":0,"short_description":5,"public_description":null}"#
            .as_bytes()
            .to_vec(),
        r#"{"base_version":0,"short_description":"x","public_description":"y","extra":1}"#
            .as_bytes()
            .to_vec(),
        r#"{"base_version":"0","short_description":null,"public_description":null}"#
            .as_bytes()
            .to_vec(),
        b"null".to_vec(),
    ];
    for body in cases {
        let (status, json, _) = send(
            &f.router,
            Method::PUT,
            &path,
            Some(&auth),
            &[(KEY_HEADER, "p-bad"), ("content-type", "application/json")],
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}: {json}");
        assert_eq!(json["error"]["code"], "invalid_request", "{body:?}");
    }

    // 缺失必填可空字段（short_description 不出现）→ invalid_request。
    let body = r#"{"base_version":0,"public_description":null}"#;
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[
            (KEY_HEADER, "p-missing"),
            ("content-type", "application/json"),
        ],
        Some(body.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // 缺失 Idempotency-Key → idempotency_key_required。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[("content-type", "application/json")],
        Some(put_profile_body(0, Some("x"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "idempotency_key_required");

    // 无认证 → 401。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        None,
        &[
            (KEY_HEADER, "p-noauth"),
            ("content-type", "application/json"),
        ],
        Some(put_profile_body(0, Some("x"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_required");
}

// ============================================================
// Room member presentation summary / detail GET
// ============================================================

#[tokio::test]
async fn member_presentations_summary_lists_all_active_members_without_full_descriptions() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;

    // Owner 为自己的 agent 写 profile 与 presentation。
    let auth = format!("Bearer {}", f.token_a);
    send(
        &f.router,
        Method::PUT,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&auth),
        &[(KEY_HEADER, "sp"), ("content-type", "application/json")],
        Some(put_profile_body(0, Some("short bio"), Some("secret public bio")).into_bytes()),
    )
    .await;

    let present_body = format!(
        r#"{{"base_version":0,"room_display_name":"Disp","room_role_name":"Role","room_role_description":"{full}"}}"#,
        full = "r".repeat(600)
    );
    let (status, pres, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/presentation"),
        Some(&auth),
        &[(KEY_HEADER, "pk"), ("content-type", "application/json")],
        Some(present_body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pres}");

    let (status, json, request_id) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/member-presentations"),
        Some(&auth),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["request_id"], request_id.unwrap());
    let members = json["members"].as_array().unwrap();
    // 摘要按 Participant 建模：active Human owner 与 active Agent 成员都在内；
    // 只有 Agent 有 presentation 字段，Human 的 presentation 字段为 null。
    assert_eq!(members.len(), 2, "{json}");
    let m = members
        .iter()
        .find(|m| m["participant"]["id"] == agent_id)
        .expect("agent member is in summary");
    assert_eq!(m["participant"]["kind"], "agent");
    assert_eq!(m["short_description"], "short bio");
    assert_eq!(m["room_display_name"], "Disp");
    assert_eq!(m["room_role_name"], "Role");
    // preview 由 full stored room_role_description 在读时刻生成，最大 512 字节。
    let preview = m["room_role_description_preview"].as_str().unwrap();
    assert_eq!(preview.len(), 512);
    let human = members
        .iter()
        .find(|m| m["participant"]["id"] == f.human_a)
        .expect("active human member is in summary");
    assert_eq!(human["participant"]["kind"], "human");
    assert_eq!(human["participant"]["status"], "active");
    assert!(human["short_description"].is_null(), "{human:?}");
    assert!(human["room_display_name"].is_null(), "{human:?}");
    assert!(human["room_role_name"].is_null(), "{human:?}");
    assert!(
        human["room_role_description_preview"].is_null(),
        "{human:?}"
    );
    assert!(
        !json.to_string().contains("secret public bio"),
        "summary must never include public_description: {json}"
    );
    for m in members {
        assert!(
            m.get("public_description").is_none() && m.get("room_role_description").is_none(),
            "summary must not expose full role description or public_description: {json}"
        );
    }
}

#[tokio::test]
async fn member_presentations_require_active_member_and_nonmember_404() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;
    let _ = agent_id;

    // Bob（非成员）看不到 summary → 404 room_not_found。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/member-presentations"),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // 未知 room → 404。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms/00000000-0000-0000-0000-000000000000/member-presentations",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");
}

#[tokio::test]
async fn member_profile_detail_returns_profile_and_presentation_for_active_agent() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;
    let auth = format!("Bearer {}", f.token_a);

    send(
        &f.router,
        Method::PUT,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&auth),
        &[(KEY_HEADER, "dp"), ("content-type", "application/json")],
        Some(put_profile_body(0, Some("bio"), Some("pub")).into_bytes()),
    )
    .await;
    let present_body = r#"{"base_version":0,"room_display_name":"D","room_role_name":"R","room_role_description":"RD"}"#;
    let (status, _, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/presentation"),
        Some(&auth),
        &[(KEY_HEADER, "dpk"), ("content-type", "application/json")],
        Some(present_body.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, json, request_id) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/profile"),
        Some(&auth),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["request_id"], request_id.unwrap());
    assert_eq!(json["participant"]["id"], agent_id);
    assert_eq!(json["profile"]["short_description"], "bio");
    assert_eq!(json["profile"]["published_by_kind"], "human");
    assert_eq!(json["presentation"]["room_display_name"], "D");
    assert_eq!(json["presentation"]["presentation_version"], 1);

    // 非成员 target / 非成员 requester → 404。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");
}

#[tokio::test]
async fn member_profile_unknown_and_left_member_are_404() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;
    let auth = format!("Bearer {}", f.token_a);

    // 未知目标 → member_not_found。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members/00000000-0000-0000-0000-000000000000/profile"),
        Some(&auth),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "member_not_found");

    // 把 agent 移出群 → 目标不再是 active 成员 → member_not_found。
    let (status, _, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{room_id}/members/{agent_id}"),
        Some(&auth),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/profile"),
        Some(&auth),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "member_not_found");
}

#[tokio::test]
async fn direct_room_member_presentation_is_null() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    // direct room（agent member）。
    let body = format!(r#"{{"agent_id":"{agent_id}"}}"#);
    let (status, room, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "dir-k"), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{room}");
    let direct_room = room["room"]["id"].as_str().unwrap().to_owned();

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{direct_room}/members/{agent_id}/presentation"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(
        json["presentation"].is_null(),
        "direct presentation must be null"
    );
}

// ============================================================
// Group Room Presentation PUT
// ============================================================

fn present_body(
    base_version: i64,
    display: Option<&str>,
    role: Option<&str>,
    desc: Option<&str>,
) -> String {
    let d = display
        .map(|s| format!(r#""{s}""#))
        .unwrap_or_else(|| "null".to_owned());
    let r = role
        .map(|s| format!(r#""{s}""#))
        .unwrap_or_else(|| "null".to_owned());
    let de = desc
        .map(|s| format!(r#""{s}""#))
        .unwrap_or_else(|| "null".to_owned());
    format!(
        r#"{{"base_version":{base_version},"room_display_name":{d},"room_role_name":{r},"room_role_description":{de}}}"#
    )
}

#[tokio::test]
async fn presentation_put_create_update_replay_and_stale_for_group_owner() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;
    let path = format!("/v1/rooms/{room_id}/members/{agent_id}/presentation");
    let auth = format!("Bearer {}", f.token_a);

    let (status, first, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "pr-1"), ("content-type", "application/json")],
        Some(present_body(0, Some("D"), Some("R"), Some("RD")).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["presentation"]["presentation_version"], 1);
    assert_eq!(first["idempotency_replayed"], false);
    assert_eq!(first["presentation"]["published_by_kind"], "human");

    let (status, second, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "pr-2"), ("content-type", "application/json")],
        Some(present_body(1, Some("D2"), None, None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["presentation"]["presentation_version"], 2);

    let (status, replay, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "pr-1"), ("content-type", "application/json")],
        Some(present_body(0, Some("D"), Some("R"), Some("RD")).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{replay}");
    assert_eq!(replay["presentation"]["presentation_version"], 1);
    assert_eq!(replay["idempotency_replayed"], true);

    // stale new key.
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &path,
        Some(&auth),
        &[(KEY_HEADER, "pr-s"), ("content-type", "application/json")],
        Some(present_body(0, Some("X"), None, None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["error"]["code"], "room_presentation_stale");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_presentation_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn presentation_put_requires_group_owner_and_active_agent_member() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;

    // Bob 不是 group owner → room_not_found（隐藏存在性）。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/presentation"),
        Some(&format!("Bearer {}", f.token_b)),
        &[
            (KEY_HEADER, "p-owner"),
            ("content-type", "application/json"),
        ],
        Some(present_body(0, Some("D"), None, None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // 目标不是该群的 active Agent 成员（Owner 自己）→ member_not_found。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_id}/members/{}/presentation", f.human_a),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            (KEY_HEADER, "p-target"),
            ("content-type", "application/json"),
        ],
        Some(present_body(0, Some("D"), None, None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "member_not_found");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_presentation_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "failed writes must not leave presentations");
}

#[tokio::test]
async fn agent_owner_without_room_membership_has_no_bypass() {
    let f = fixture().await;
    let outside_agent = create_agent(&f.router, &f.token_a, "外部助手", "oa").await;
    let outside_id = outside_agent["agent"]["id"].as_str().unwrap().to_owned();
    let group = create_group(&f.router, &f.token_a, "g", "gg").await;
    let room_id = group["room"]["id"].as_str().unwrap().to_owned();

    // Owner 拥有该 agent 且是 group owner，但该 agent 不是 group 成员。
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_id}/members/{outside_id}/presentation"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            (KEY_HEADER, "p-outsider"),
            ("content-type", "application/json"),
        ],
        Some(present_body(0, Some("D"), None, None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "member_not_found");
}

#[tokio::test]
async fn direct_room_presentation_put_is_unsupported() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let body = format!(r#"{{"agent_id":"{agent_id}"}}"#);
    let (status, room, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "d"), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{room}");
    let direct_room = room["room"]["id"].as_str().unwrap().to_owned();

    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{direct_room}/members/{agent_id}/presentation"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            (KEY_HEADER, "p-direct"),
            ("content-type", "application/json"),
        ],
        Some(present_body(0, Some("D"), None, None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "room_presentation_not_supported");
}

#[tokio::test]
async fn two_group_rooms_have_independent_presentation_versions_and_null_display_name_preserved() {
    let f = fixture().await;
    let (agent_id, room_a) = seeded_world(&f).await;
    let room_b_raw = create_group(&f.router, &f.token_a, "second group", "g2").await;
    let room_b = room_b_raw["room"]["id"].as_str().unwrap().to_owned();
    let (status, _) = invite_member(&f.router, &f.token_a, &room_b, &agent_id, "inv-b").await;
    assert_eq!(status, StatusCode::CREATED);
    let auth = format!("Bearer {}", f.token_a);

    // room A 写 v2（先 v1 再 v2），保持 room_display_name=null（not-set 事实）。
    send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_a}/members/{agent_id}/presentation"),
        Some(&auth),
        &[(KEY_HEADER, "a1"), ("content-type", "application/json")],
        Some(present_body(0, Some("A-D"), None, None).into_bytes()),
    )
    .await;
    let (status, a2, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_a}/members/{agent_id}/presentation"),
        Some(&auth),
        &[(KEY_HEADER, "a2"), ("content-type", "application/json")],
        Some(present_body(1, None, Some("A-Role"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{a2}");
    assert_eq!(a2["presentation"]["presentation_version"], 2);
    assert!(
        a2["presentation"]["room_display_name"].is_null(),
        "null room_display_name must stay null (inherit room name on the client, never invented)"
    );
    assert_eq!(a2["presentation"]["room_role_name"], "A-Role");

    // room B 独立版本链（自己的 v1）。
    let (status, b1, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{room_b}/members/{agent_id}/presentation"),
        Some(&auth),
        &[(KEY_HEADER, "b1"), ("content-type", "application/json")],
        Some(present_body(0, Some("B-D"), Some("B-Role"), None).into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{b1}");
    assert_eq!(b1["presentation"]["presentation_version"], 1);
    // 两个 Group Room 各自独立的版本链：room A 的 v2 与 room B 的 v1 并存，
    // 互不干扰（版本单调按 (room, participant) 各自推进）。
}

#[tokio::test]
async fn existing_members_shape_unchanged() {
    let f = fixture().await;
    let (agent_id, room_id) = seeded_world(&f).await;
    // members 列表响应 shape：membership + participant（无 presentation 字段）。
    let (status, members, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    let arr = members["members"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    for item in arr {
        assert!(item.get("membership").is_some(), "{item}");
        assert_eq!(
            item["membership"].as_object().unwrap().len(),
            6,
            "membership view must keep its existing shape"
        );
        assert!(item.get("short_description").is_none(), "{item}");
        assert!(item.get("presentation").is_none(), "{item}");
    }
    assert!(
        arr.iter()
            .any(|i| i["participant"]["id"].as_str() == Some(agent_id.as_str()))
    );
}
// ---------------------------------------------------------------------------
// 表达权第一刀（批次四）：Agent 写**自己**的资料
// ---------------------------------------------------------------------------

/// 给一支 Agent 发一份 Backend 凭据（走公开管理面：binding → credential）。
async fn agent_credential(
    router: &axum::Router,
    owner_token: &str,
    agent_id: &str,
    key: &str,
) -> String {
    let body = format!(r#"{{"label":"{key}"}}"#);
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(&format!("Bearer {owner_token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body.into_bytes()),
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

/// **一支 Agent 写自己的资料**（批次四表达权第一刀）：在此之前它对自己的介绍
/// 只能由 Owner 代笔。判据是并集——Owner 的写权限一字不动，多了"它自己"这一支。
#[tokio::test]
async fn an_agent_writes_its_own_profile_via_backend_auth() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "小满", "exp-a").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let credential = agent_credential(&f.router, &f.token_a, &agent_id, "exp-b").await;

    // 首次：base_version 0 ⇒ 201。
    let body = serde_json::json!({"base_version": 0, "short_description": "我是小满", "public_description": "我在这里长期住着。"}).to_string().into_bytes();
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        "/v1/backend/me/profile",
        Some(&format!("Bearer {credential}")),
        &[(KEY_HEADER, "exp-p1"), ("content-type", "application/json")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "它应当能写自己的资料: {json}");
    assert_eq!(json["profile"]["short_description"], "我是小满");

    // 第二次：base_version 1 ⇒ 200（版本严格 +1，追加新版本，不改写既有行）。
    let body = serde_json::json!({"base_version": 1, "short_description": "我是小满（改过）", "public_description": "我在这里长期住着。"}).to_string().into_bytes();
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        "/v1/backend/me/profile",
        Some(&format!("Bearer {credential}")),
        &[(KEY_HEADER, "exp-p2"), ("content-type", "application/json")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");

    // 世界侧核对：Owner 那条既有读路径读得到它自己写的那一版。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}/profile"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["profile"]["short_description"], "我是小满（改过）");
}

/// **写路径不提供读**：`/v1/backend/me/profile` 是**写**路径（只注册了 `PUT`），
/// 「看见自己」那条**读**路径是 `/v1/backend/profile`（`CORE_V1_AGENT_READ_SURFACE.md` §2.2）。
///
/// ⚠ 为什么值得一条专门的桩（2026-09-29 真机实测的教训）：执行端曾经把**读**打到这条写路径上
/// ⇒ 本项目实测 `405 method_not_allowed` ⇒ 执行端归入 `unknown` ⇒ 整次活动 `system:indeterminate`
/// ⇒ 因为写之前要先读 `base_version`，**Agent 连自己的资料都写不了**，而两边门禁各自全绿。
/// 事后最容易走偏的"修法"是**给这条写路径补一个 GET** —— 那会让实现与契约 §2.2 的路径
/// **分叉**（读面就变成了两条路）。这条用例把那个岔路口堵死：**写路径上 GET 必须是 405**。
#[tokio::test]
async fn the_write_route_does_not_answer_get_because_the_read_route_is_elsewhere() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "小满", "exp-a").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let credential = agent_credential(&f.router, &f.token_a, &agent_id, "exp-b").await;

    let (status, _, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/me/profile",
        Some(&format!("Bearer {credential}")),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "写路径上出现 GET，说明有人把读面接到了写路径上 —— 读面在 /v1/backend/profile"
    );

    // 正对照：契约 §2.2 那条读路径**必须**答 GET（否则这条桩就变成了"哪边都不通"的假绿）。
    let (status, _, _) = send(
        &f.router,
        Method::GET,
        "/v1/backend/profile",
        Some(&format!("Bearer {credential}")),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "「看见自己」那条读路径必须存在且答 GET"
    );
}

/// 表达面与读取面同一条纪律：**身份只来自认证**，且必须是 BackendAuth。
#[tokio::test]
async fn backend_profile_write_requires_backend_auth() {
    let f = fixture().await;
    let body = serde_json::json!({"base_version": 0, "short_description": "x", "public_description": null}).to_string().into_bytes();
    for (label, authorization, expected) in [
        ("无凭据", None, "authentication_required"),
        ("Human token", Some(f.token_a.clone()), "invalid_credential"),
    ] {
        let (status, json, _) = send(
            &f.router,
            Method::PUT,
            "/v1/backend/me/profile",
            authorization
                .as_deref()
                .map(|t| format!("Bearer {t}"))
                .as_deref(),
            &[(KEY_HEADER, "exp-x"), ("content-type", "application/json")],
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}: {json}");
        assert_eq!(json["error"]["code"], expected, "{label}");
    }
}
// ---------------------------------------------------------------------------
// 表达权第三刀（批次四）：Agent 写**自己**在群房里的呈现
// ---------------------------------------------------------------------------

/// **一支 Agent 写自己在群里的那一份呈现**（表达权第三刀）：在此之前只能由房 owner 代笔。
#[tokio::test]
async fn an_agent_writes_its_own_room_presentation_via_backend_auth() {
    let f = fixture().await;
    let agent = create_agent(&f.router, &f.token_a, "小满", "expr-pres-a").await;
    let agent_id = agent["agent"]["id"].as_str().unwrap().to_owned();
    let group = create_group(&f.router, &f.token_a, "验收群", "expr-pres-g").await;
    let room_id = group["room"]["id"].as_str().unwrap().to_owned();
    // owner（主人）把它拉进群（归属是资格之一，与批次三第③步同形）。
    let (status, json) =
        invite_member(&f.router, &f.token_a, &room_id, &agent_id, "expr-pres-i").await;
    assert_eq!(status, StatusCode::CREATED, "invite: {json}");

    let credential = agent_credential(&f.router, &f.token_a, &agent_id, "expr-pres-c").await;
    let body = serde_json::json!({
        "base_version": 0,
        "room_display_name": "小满",
        "room_role_name": "记账的",
        "room_role_description": "在这间房里，我负责把说过的事记下来。"
    })
    .to_string()
    .into_bytes();
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/backend/rooms/{room_id}/my-presentation"),
        Some(&format!("Bearer {credential}")),
        &[
            (KEY_HEADER, "expr-pres-1"),
            ("content-type", "application/json"),
        ],
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "它应当能写自己在群里的呈现: {json}"
    );
    assert_eq!(json["presentation"]["presentation_version"], 1);
    assert_eq!(json["presentation"]["room_role_name"], "记账的");

    // 世界侧核对：房 owner 那条既有读路径读得到它自己写的那一份。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members/{agent_id}/presentation"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["presentation"]["room_role_name"], "记账的");
}
