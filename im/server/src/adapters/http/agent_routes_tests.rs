//! Agent HTTP 契约测试（CORE_V1_AGENT_OWNERSHIP.md 5.x/6.x）：创建、
//! 幂等、列表、读取、撤销、错误信封与黑盒状态码矩阵。

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
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session。
///
/// bootstrap guard 是应用层「首位 Human」门禁，只允许一次 bootstrap；
/// participants 表本身允许并存多个 human 行（存储事实），这里用最接近
/// 生产数据的路径构造第二个 Human，供跨 Human 隔离场景使用。
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
        Arc::new(identity.clone()),
        Arc::new(agent),
        Arc::new(room),
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
) -> (StatusCode, serde_json::Value, Option<String>) {
    let body = format!(r#"{{"display_name":"{display_name}"}}"#);
    send(
        router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {token}")),
        &[(KEY_HEADER, key), ("content-type", "application/json")],
        Some(body.into_bytes()),
    )
    .await
}

#[tokio::test]
async fn create_returns_201_with_agent_ownership_and_no_secrets() {
    let f = fixture().await;
    let (status, json, request_id) = create_agent(&f.router, &f.token_a, "助手", "key-1").await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["idempotency_replayed"], false);

    let agent = &json["agent"];
    let agent_id = agent["id"].as_str().unwrap();
    assert_ne!(agent_id, f.human_a, "agent id must differ from human id");
    assert_eq!(agent["kind"], "agent");
    assert_eq!(agent["display_name"], "助手");
    assert_eq!(agent["status"], "active");
    assert!(agent["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(agent["revoked_at"].is_null());

    let ownership = &json["ownership"];
    assert_eq!(ownership["agent_id"], agent_id);
    assert_eq!(ownership["human_id"], f.human_a);
    assert_eq!(ownership["status"], "active");
    assert!(ownership["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(ownership["revoked_at"].is_null());

    assert_eq!(json["request_id"], request_id.unwrap());
    let keys: std::collections::BTreeSet<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["agent", "ownership", "idempotency_replayed", "request_id"]
            .into_iter()
            .collect(),
        "no backend/room/message fields may leak into the response"
    );

    let me = send(
        &f.router,
        Method::GET,
        "/v1/me",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(me.0, StatusCode::OK);
    assert_eq!(
        me.1["participant"]["id"], f.human_a,
        "human identity unchanged"
    );
}

#[tokio::test]
async fn create_replay_same_key_payload_returns_200_same_agent() {
    let f = fixture().await;
    let (status, first_json, _) = create_agent(&f.router, &f.token_a, "助手", "key-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let first_id = first_json["agent"]["id"].as_str().unwrap().to_owned();

    let (status, replay_json, _) = create_agent(&f.router, &f.token_a, "助手", "key-1").await;
    assert_eq!(status, StatusCode::OK, "{replay_json}");
    assert_eq!(replay_json["idempotency_replayed"], true);
    assert_eq!(replay_json["agent"]["id"], first_id);
    assert_eq!(replay_json["ownership"]["human_id"], f.human_a);

    let (list_status, list_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(list_json["agents"].as_array().unwrap().len(), 1);
    assert_eq!(list_json["agents"][0]["agent"]["id"], first_id);
}

#[tokio::test]
async fn create_same_key_different_payload_returns_409_and_keeps_first_facts() {
    let f = fixture().await;
    let (status, first_json, _) = create_agent(&f.router, &f.token_a, "助手", "key-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let first_id = first_json["agent"]["id"].as_str().unwrap().to_owned();

    let (status, conflict_json, request_id) =
        create_agent(&f.router, &f.token_a, "另一个助手", "key-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict_json["error"]["code"], "idempotency_conflict");
    assert_eq!(conflict_json["error"]["request_id"], request_id.unwrap());

    let (_, replay_json, _) = create_agent(&f.router, &f.token_a, "助手", "key-1").await;
    assert_eq!(
        replay_json["agent"]["id"], first_id,
        "first facts must survive"
    );
}

#[tokio::test]
async fn create_same_key_different_humans_both_succeed_and_are_isolated() {
    let f = fixture().await;
    let (status_a, json_a, _) = create_agent(&f.router, &f.token_a, "A 助手", "shared-key").await;
    assert_eq!(status_a, StatusCode::CREATED);
    let (status_b, json_b, _) = create_agent(&f.router, &f.token_b, "B 助手", "shared-key").await;
    assert_eq!(
        status_b,
        StatusCode::CREATED,
        "different human + same key is not a conflict"
    );
    assert_ne!(json_a["agent"]["id"], json_b["agent"]["id"]);

    let (_, list_a, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    let (_, list_b, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(list_a["agents"].as_array().unwrap().len(), 1);
    assert_eq!(list_a["agents"][0]["agent"]["id"], json_a["agent"]["id"]);
    assert_eq!(list_b["agents"].as_array().unwrap().len(), 1);
    assert_eq!(list_b["agents"][0]["agent"]["id"], json_b["agent"]["id"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn create_concurrent_same_request_creates_exactly_one_agent() {
    let f = fixture().await;
    let router = Arc::new(f.router);
    let token = f.token_a.clone();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let router = router.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            create_agent(&router, &token, "并发", "concurrent-key").await
        }));
    }
    let mut created = 0;
    let mut replayed = 0;
    for task in tasks {
        let (status, json, _) = task.await.unwrap();
        match status.as_u16() {
            201 => created += 1,
            200 => {
                assert_eq!(json["idempotency_replayed"], true);
                replayed += 1;
            }
            other => panic!("unexpected concurrent create status {other}: {json}"),
        }
    }
    assert_eq!(created, 1);
    assert_eq!(replayed, 7);

    let (_, list_json, _) = send(
        &router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {token}")),
        &[],
        None,
    )
    .await;
    assert_eq!(list_json["agents"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn create_missing_key_returns_400_required_without_side_effects() {
    let f = fixture().await;
    let (status, json, request_id) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json")],
        Some(r#"{"display_name":"助手"}"#.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "idempotency_key_required");
    assert_eq!(json["error"]["request_id"], request_id.unwrap());

    let (_, list_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(list_json["agents"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_invalid_keys_are_400_invalid_without_side_effects() {
    let f = fixture().await;
    let body = r#"{"display_name":"助手"}"#.as_bytes().to_vec();
    let long_key = "k".repeat(129);
    let mut cases: Vec<(Vec<(&str, &str)>, String)> = Vec::new();
    // 注：控制字符（如 NUL）不是合法的 HTTP header 值，会在 HTTP 层被
    // 拒绝，无法进入应用；这里覆盖所有「可被 HTTP 传输」的非法 key 形态。
    for key in ["", "a b", " a", "a ", "a,b", "é", long_key.as_str()] {
        cases.push((
            vec![(KEY_HEADER, key), ("content-type", "application/json")],
            format!("invalid key {key:?}"),
        ));
    }
    cases.push((
        vec![
            (KEY_HEADER, "dup-key"),
            (KEY_HEADER, "dup-key"),
            ("content-type", "application/json"),
        ],
        "duplicate key header".into(),
    ));
    cases.push((
        vec![(KEY_HEADER, "k1,k2"), ("content-type", "application/json")],
        "comma-merged key header".into(),
    ));

    for (headers, label) in cases {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            "/v1/agents",
            Some(&format!("Bearer {}", f.token_a)),
            &headers,
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{label}: expected 400, got {json}"
        );
        assert_eq!(
            json["error"]["code"], "invalid_idempotency_key",
            "{label}: {json}"
        );
    }

    let (_, list_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(
        list_json["agents"].as_array().unwrap().len(),
        0,
        "invalid keys must not create anything"
    );
}

#[tokio::test]
async fn create_invalid_display_names_are_400_invalid_request() {
    let f = fixture().await;
    let bad_names = ["   ", "\t\n ", "al\u{0}ice", &"a".repeat(81)];
    for name in bad_names {
        let (status, json, _) = create_agent(&f.router, &f.token_a, name, "ok-key").await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "expected 400 for {name:?}, got {json}"
        );
        assert_eq!(json["error"]["code"], "invalid_request", "{json}");
    }

    let (_, list_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(list_json["agents"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_malformed_unknown_or_missing_field_json_is_400_invalid_request() {
    let f = fixture().await;
    let bodies: Vec<Vec<u8>> = vec![
        b"not json at all".to_vec(),
        b"{}".to_vec(),
        b"{\"display_name\":5}".to_vec(),
        "{\"display_name\":\"助手\",\"extra\":1}"
            .as_bytes()
            .to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{\"display_name\":}".to_vec(),
    ];
    for body in bodies {
        let (status, json, request_id) = send(
            &f.router,
            Method::POST,
            "/v1/agents",
            Some(&format!("Bearer {}", f.token_a)),
            &[(KEY_HEADER, "ok-key"), ("content-type", "application/json")],
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "expected 400 for {body:?}, got {status} {json}"
        );
        assert_eq!(json["error"]["code"], "invalid_request", "{json}");
        assert_eq!(json["error"]["request_id"], request_id.unwrap());
    }

    let (_, list_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(list_json["agents"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_body_at_4096_bytes_passes_and_over_limit_is_413() {
    let f = fixture().await;
    let auth = format!("Bearer {}", f.token_a);

    let mut at_limit = b"{\"display_name\":\"a\"".to_vec();
    at_limit.extend(std::iter::repeat_n(b' ', 4096 - at_limit.len() - 1));
    at_limit.push(b'}');
    assert_eq!(at_limit.len(), 4096);

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&auth),
        &[
            (KEY_HEADER, "boundary-ok"),
            ("content-type", "application/json"),
        ],
        Some(at_limit),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "4096 bytes of valid JSON must pass the limit: {json}"
    );

    let mut over_limit = b"{\"display_name\":\"b\"".to_vec();
    over_limit.extend(std::iter::repeat_n(b' ', 4096 - over_limit.len() - 1));
    over_limit.push(b'}');
    over_limit.push(b' ');
    assert_eq!(over_limit.len(), 4097);

    let (status, json, request_id) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&auth),
        &[
            (KEY_HEADER, "boundary-over"),
            ("content-type", "application/json"),
        ],
        Some(over_limit),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{json}");
    assert_eq!(json["error"]["code"], "payload_too_large");
    assert_eq!(json["error"]["request_id"], request_id.unwrap());

    let (_, list_json, _) =
        send(&f.router, Method::GET, "/v1/agents", Some(&auth), &[], None).await;
    assert_eq!(
        list_json["agents"].as_array().unwrap().len(),
        1,
        "only the first create may exist"
    );
}

#[tokio::test]
async fn create_requires_valid_human_session() {
    let f = fixture().await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        None,
        &[(KEY_HEADER, "k"), ("content-type", "application/json")],
        Some(r#"{"display_name":"助手"}"#.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some("Bearer clr_hs_AAAA"),
        &[(KEY_HEADER, "k"), ("content-type", "application/json")],
        Some(r#"{"display_name":"助手"}"#.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");
}

#[tokio::test]
async fn list_returns_only_owned_agents_in_stable_order() {
    let f = fixture().await;
    for (name, key) in [("一", "k-1"), ("二", "k-2"), ("三", "k-3")] {
        assert_eq!(
            create_agent(&f.router, &f.token_a, name, key).await.0,
            StatusCode::CREATED
        );
    }
    create_agent(&f.router, &f.token_b, "B 的", "k-b").await;

    let (status, json, request_id) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["request_id"], request_id.unwrap());

    let agents = json["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 3);
    let mut expected: Vec<&serde_json::Value> = agents.iter().collect();
    expected.sort_by(|a, b| {
        (a["agent"]["created_at"].as_str(), a["agent"]["id"].as_str())
            .cmp(&(b["agent"]["created_at"].as_str(), b["agent"]["id"].as_str()))
    });
    let actual: Vec<&serde_json::Value> = agents.iter().collect();
    assert_eq!(
        actual, expected,
        "agents must be sorted by created_at then id"
    );
    for item in agents {
        assert_eq!(item["ownership"]["human_id"], f.human_a);
        assert_eq!(item["agent"]["kind"], "agent");
    }
    assert!(json.get("backend").is_none());
    assert!(json.get("rooms").is_none());
}

#[tokio::test]
async fn list_empty_for_owner_without_agents_returns_200() {
    let f = fixture().await;
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agents"], serde_json::json!([]));
}

#[tokio::test]
async fn get_own_returns_agent_and_ownership() {
    let f = fixture().await;
    let (_, created, _) = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = created["agent"]["id"].as_str().unwrap();

    let (status, json, request_id) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["agent"]["id"], agent_id);
    assert_eq!(json["agent"]["kind"], "agent");
    assert_eq!(json["agent"]["display_name"], "助手");
    assert_eq!(json["agent"]["status"], "active");
    assert_eq!(json["ownership"]["human_id"], f.human_a);
    assert_eq!(json["request_id"], request_id.unwrap());
}

#[tokio::test]
async fn get_unknown_bad_id_and_not_owned_are_uniform_404() {
    let f = fixture().await;
    let (_, created, _) = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = created["agent"]["id"].as_str().unwrap().to_owned();

    let paths = [
        "/v1/agents/00000000-0000-0000-0000-000000000000".to_owned(),
        "/v1/agents/not-a-uuid".to_owned(),
        format!("/v1/agents/{agent_id}"),
    ];
    for path in paths {
        let token = if path.ends_with(&agent_id) {
            &f.token_b
        } else {
            &f.token_a
        };
        let (status, json, request_id) = send(
            &f.router,
            Method::GET,
            &path,
            Some(&format!("Bearer {token}")),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {json}");
        assert_eq!(json["error"]["code"], "agent_not_found", "{path}");
        assert_eq!(json["error"]["request_id"], request_id.unwrap());
    }
}

#[tokio::test]
async fn delete_revokes_atomically_and_keeps_human_session() {
    let f = fixture().await;
    let (_, created, _) = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = created["agent"]["id"].as_str().unwrap().to_owned();

    let (status, json, request_id) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/agents/{agent_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");
    assert_eq!(json.get("error"), None);
    assert!(request_id.is_some(), "204 must still carry x-request-id");

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent"]["status"], "revoked");
    assert!(!json["agent"]["revoked_at"].is_null());
    assert_eq!(json["ownership"]["status"], "revoked");
    assert!(!json["ownership"]["revoked_at"].is_null());

    let (_, list_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(list_json["agents"][0]["agent"]["status"], "revoked");

    let (status, me_json, _) = send(
        &f.router,
        Method::GET,
        "/v1/me",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        me_json["participant"]["status"], "active",
        "human session must survive"
    );
}

#[tokio::test]
async fn delete_repeat_returns_204_and_keeps_original_revoked_at() {
    let f = fixture().await;
    let (_, created, _) = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = created["agent"]["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/agents/{agent_id}");
    let auth = format!("Bearer {}", f.token_a);

    let (status, _, _) = send(&f.router, Method::DELETE, &path, Some(&auth), &[], None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, after_first, _) = send(&f.router, Method::GET, &path, Some(&auth), &[], None).await;
    let first_revoked_at = after_first["agent"]["revoked_at"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_owner_revoked_at = after_first["ownership"]["revoked_at"]
        .as_str()
        .unwrap()
        .to_owned();

    let (status, json, _) = send(&f.router, Method::DELETE, &path, Some(&auth), &[], None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");

    let (_, after_repeat, _) = send(&f.router, Method::GET, &path, Some(&auth), &[], None).await;
    assert_eq!(after_repeat["agent"]["revoked_at"], first_revoked_at);
    assert_eq!(
        after_repeat["ownership"]["revoked_at"],
        first_owner_revoked_at
    );
}

#[tokio::test]
async fn delete_unknown_bad_id_and_not_owned_are_uniform_404() {
    let f = fixture().await;
    let (_, created, _) = create_agent(&f.router, &f.token_a, "助手", "k").await;
    let agent_id = created["agent"]["id"].as_str().unwrap().to_owned();

    for (token, path) in [
        (
            f.token_a.clone(),
            "/v1/agents/00000000-0000-0000-0000-000000000000".to_owned(),
        ),
        (f.token_a.clone(), "/v1/agents/not-a-uuid".to_owned()),
        (f.token_b.clone(), format!("/v1/agents/{agent_id}")),
    ] {
        let (status, json, _) = send(
            &f.router,
            Method::DELETE,
            &path,
            Some(&format!("Bearer {token}")),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {json}");
        assert_eq!(json["error"]["code"], "agent_not_found", "{path}");
    }

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/agents/{agent_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "alice's agent must be untouched");
    assert_eq!(json["agent"]["status"], "active");
}

#[tokio::test]
async fn wrong_methods_and_unknown_subpaths_return_stable_errors() {
    let f = fixture().await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents/some-id",
        Some(&format!("Bearer {}", f.token_a)),
        &[(KEY_HEADER, "k"), ("content-type", "application/json")],
        Some(r#"{"display_name":"助手"}"#.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{json}");
    assert_eq!(json["error"]["code"], "method_not_allowed");

    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        "/v1/agents",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json["error"]["code"], "method_not_allowed");

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/agents/some-id/extra",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "not_found");
}

#[tokio::test]
async fn health_and_ready_remain_anonymous_and_ok() {
    let f = fixture().await;
    let (status, json, _) = send(&f.router, Method::GET, "/health", None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");

    let (status, json, _) = send(&f.router, Method::GET, "/ready", None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
}
