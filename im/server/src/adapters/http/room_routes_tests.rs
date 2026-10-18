//! Direct Room HTTP 契约测试（CORE_V1_DIRECT_ROOMS.md 7.x/8.x）：创建、
//! 幂等重放/冲突、错误码矩阵、member-scoped 读取、Agent 撤销后可见性、
//! 请求体上限与错误信封。

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

async fn post_room(
    router: &axum::Router,
    token: &str,
    key: &str,
    body_json: serde_json::Value,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {token}")),
        &[("content-type", "application/json"), (KEY_HEADER, key)],
        Some(body_json.to_string().into_bytes()),
    )
    .await
}

/// 通过公开 API 创建 Agent 并返回其 ID。
async fn create_agent_via_api(router: &axum::Router, token: &str, name: &str, key: &str) -> String {
    let body = serde_json::json!({ "display_name": name });
    let (status, json, _) = send(
        router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {token}")),
        &[("content-type", "application/json"), (KEY_HEADER, key)],
        Some(body.to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent create must succeed");
    json["agent"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn create_returns_201_with_room_and_two_memberships() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;

    let body = serde_json::json!({ "agent_id": agent_id });
    let (status, json, request_id) = post_room(&f.router, &f.token_a, "r-k", body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["room"]["kind"], "direct");
    assert_eq!(json["room"]["status"], "active");
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(
        json["room"]["id"].as_str().unwrap().len(),
        36,
        "room id must be a uuid"
    );
    let memberships = json["memberships"].as_array().unwrap();
    assert_eq!(memberships.len(), 2);
    assert_eq!(memberships[0]["participant_id"], f.human_a);
    assert_eq!(memberships[0]["role"], "owner");
    assert_eq!(memberships[0]["left_at"], serde_json::Value::Null);
    assert_eq!(memberships[0]["closed_by"], serde_json::Value::Null);
    assert_eq!(memberships[0]["membership_id"].as_str().unwrap().len(), 36);
    assert_eq!(memberships[1]["participant_id"], agent_id);
    assert_eq!(memberships[1]["role"], "member");
    assert_eq!(memberships[1]["left_at"], serde_json::Value::Null);
    assert_eq!(memberships[1]["closed_by"], serde_json::Value::Null);
    assert!(request_id.is_some());
    assert_eq!(json["request_id"], request_id.unwrap());
}

#[tokio::test]
async fn creating_agent_does_not_create_membership_until_direct_room() {
    let f = fixture().await;
    create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["rooms"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn replay_same_key_same_agent_returns_200_same_room() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;
    let body = serde_json::json!({ "agent_id": agent_id });

    let (status, first, _) = post_room(&f.router, &f.token_a, "r-k", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let room_id = first["room"]["id"].as_str().unwrap().to_owned();

    let (status, replay, _) = post_room(&f.router, &f.token_a, "r-k", body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["idempotency_replayed"], true);
    assert_eq!(replay["room"]["id"], room_id);
    assert_eq!(
        replay["memberships"].as_array().unwrap().len(),
        2,
        "replay must not grow the room"
    );
}

#[tokio::test]
async fn same_key_different_agent_is_409_idempotency_conflict() {
    let f = fixture().await;
    let agent_a = create_agent_via_api(&f.router, &f.token_a, "甲", "a-k1").await;
    let agent_b = create_agent_via_api(&f.router, &f.token_a, "乙", "a-k2").await;
    let body_a = serde_json::json!({ "agent_id": agent_a });
    let (status, _, _) = post_room(&f.router, &f.token_a, "r-k", body_a).await;
    assert_eq!(status, StatusCode::CREATED);

    let body_b = serde_json::json!({ "agent_id": agent_b });
    let (status, json, request_id) = post_room(&f.router, &f.token_a, "r-k", body_b).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");
    assert_eq!(json["error"]["request_id"], request_id.unwrap());

    let (status, _replay, _) = post_room(
        &f.router,
        &f.token_a,
        "r-k",
        serde_json::json!({ "agent_id": agent_a }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "first facts must not be overwritten"
    );
}

#[tokio::test]
async fn different_keys_same_pair_replay_the_same_room() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "甲", "a-k").await;
    let body = serde_json::json!({ "agent_id": agent_id });
    let (status, first, _) = post_room(&f.router, &f.token_a, "r-k1", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let room_1 = first["room"]["id"].as_str().unwrap().to_owned();

    // 批次二「一间房 = 一段关系」：同一 Human–Agent 对**只有一间私人房**
    //（每段 Agent Owner 关系一间）。换个 key 再要一次 ⇒ **200 重放原房**：
    // 不新开、也不报错——IM 里「再点一次同一个联系人」就是回到你们那间房。
    let (status, second, _) = post_room(&f.router, &f.token_a, "r-k2", body).await;
    assert_eq!(status, StatusCode::OK, "second request must replay: 200");
    assert_eq!(second["idempotency_replayed"], true);
    assert_eq!(
        second["room"]["id"].as_str().unwrap(),
        room_1,
        "replay returns the original room"
    );

    // list 只一间；get 正常，而且**没有标题**（direct 房没有标题）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rooms = json["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 1, "只有一间房：{json}");
    assert_eq!(rooms[0]["room"]["id"].as_str().unwrap(), room_1);

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_1}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["room"]["id"], serde_json::json!(room_1));
    assert_eq!(
        json["room"]["name"],
        serde_json::Value::Null,
        "direct 房没有标题"
    );
}

// （批次二删除）`named_rooms_round_trip_title_and_idempotency` 整篇的主语是
// 「可选标题的往返与幂等」——direct 房已**没有标题**（建房也不再收 `name`），
// 该主语不存在了。它顺带覆盖的「同 key 重放回原房」由
// `different_keys_same_pair_replay_the_same_room`（本文件）与存储层
// `room_store_tests::replay_same_key_matches_agent` 覆盖。

#[tokio::test]
async fn direct_room_rejects_unknown_fields_including_name() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "甲", "a-k").await;

    // 批次二：direct 房**没有标题**——`name` 不再是这个端点的合法字段。
    // 既有口径是「未知字段一律拒绝」（`deny_unknown_fields`）⇒ 400 invalid_request。
    for body in [
        serde_json::json!({ "agent_id": agent_id, "name": "项目A" }),
        serde_json::json!({ "agent_id": agent_id, "name": serde_json::Value::Null }),
        serde_json::json!({ "agent_id": agent_id, "unexpected": 1 }),
    ] {
        let (status, json, _) = post_room(&f.router, &f.token_a, "k-unknown", body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "unknown field must be rejected: {json}"
        );
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    // 只带 agent_id 的合法请求仍然成立，且房间**没有标题**。
    let (status, json, _) = post_room(
        &f.router,
        &f.token_a,
        "k-ok",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {json}");
    assert_eq!(json["room"]["name"], serde_json::Value::Null);
}
#[tokio::test]
async fn unknown_not_owned_and_bad_agent_ids_are_uniform_404_agent_not_found() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "甲的", "a-k1").await;
    let unknown = "00000000-0000-0000-0000-000000000000";

    for (token, agent) in [
        (&f.token_a, unknown),           // unknown
        (&f.token_a, "not-a-uuid"),      // bad id
        (&f.token_b, agent_id.as_str()), // not owned
        (&f.token_a, ""),                // bad id
    ] {
        let (status, json, _) = post_room(
            &f.router,
            token,
            "fresh-key",
            serde_json::json!({ "agent_id": agent }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "agent {agent:?}");
        assert_eq!(json["error"]["code"], "agent_not_found", "agent {agent:?}");
    }
}

/// **第③步补上的那一支**：Human 与"自己并不拥有的 Agent"建立 accepted 关系之后，
/// 终于有了开房路径——消掉批次三第二刀登记的那个缺口。
///
/// 判据是**并集**（主人 2026-09-27 裁决：「像飞书 QQ 那种，Agent 要进来，肯定是
/// 人类用户拉他进来得」）：**归属 或 关系**，与群邀请资格同形。
#[tokio::test]
async fn an_accepted_relation_also_admits_a_private_room() {
    let f = fixture().await;
    // 甲拥有一支 Agent；乙与它既无归属、也无关系。
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "甲的", "a-rel").await;

    // 没有关系时：乙开不出房（统一 404，不泄露 Agent 是否存在）。
    let (status, json, _) = post_room(
        &f.router,
        &f.token_b,
        "rel-k0",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");

    // 乙与它有一段 accepted 关系（夹具种下；产品路径是"申请 → 它自己接受"）。
    crate::adapters::sqlite::test_relations::seed_accepted_relation_for_token(
        &f.pool, &f.token_b, &agent_id,
    )
    .await;

    let (status, json, _) = post_room(
        &f.router,
        &f.token_b,
        "rel-k1",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "关系应当够用: {json}");
    assert!(json["room"]["id"].as_str().is_some(), "{json}");
}

#[tokio::test]
async fn missing_and_invalid_idempotency_keys_are_rejected() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;
    let body = serde_json::json!({ "agent_id": agent_id })
        .to_string()
        .into_bytes();

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "idempotency_key_required");

    for key in ["", " a", "a b", "a,b", &"a".repeat(129)] {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            "/v1/direct-rooms",
            Some(&format!("Bearer {}", f.token_a)),
            &[("content-type", "application/json"), (KEY_HEADER, key)],
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "key {key:?}");
        assert_eq!(
            json["error"]["code"], "invalid_idempotency_key",
            "key {key:?}"
        );
    }
}

#[tokio::test]
async fn malformed_json_unknown_field_and_oversized_body_are_rejected() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;

    let (status, json, _) = post_room(
        &f.router,
        &f.token_a,
        "k",
        serde_json::json!({ "agent_id": agent_id, "extra": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    let (status, json, _) = post_room(&f.router, &f.token_a, "k", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "k")],
        Some(b"{\"agent_id\":\"".to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");

    let oversized = format!("{{\"agent_id\":\"{}\"}}", "a".repeat(8192));
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "k")],
        Some(oversized.into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn unauthenticated_room_requests_fail_closed() {
    let f = fixture().await;
    let (status, json, _) = send(&f.router, Method::GET, "/v1/rooms", None, &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/direct-rooms",
        None,
        &[(KEY_HEADER, "k")],
        Some(b"{\"agent_id\":\"x\"}".to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");
}

#[tokio::test]
async fn list_get_and_members_are_member_scoped() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;
    let body = serde_json::json!({ "agent_id": agent_id });
    let (status, create_json, _) = post_room(&f.router, &f.token_a, "r-k", body).await;
    assert_eq!(status, StatusCode::CREATED);
    let room_id = create_json["room"]["id"].as_str().unwrap().to_owned();

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rooms = json["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0]["room"]["id"], room_id);
    assert_eq!(rooms[0]["room"]["kind"], "direct");
    assert_eq!(rooms[0]["membership"]["participant_id"], f.human_a);
    assert_eq!(rooms[0]["membership"]["role"], "owner");
    assert_eq!(rooms[0]["peer"]["id"], agent_id);
    assert_eq!(rooms[0]["peer"]["kind"], "agent");
    assert_eq!(rooms[0]["peer"]["display_name"], "助手");
    assert_eq!(rooms[0]["peer"]["status"], "active");

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["room"]["id"], room_id);
    assert_eq!(json["membership"]["participant_id"], f.human_a);
    assert_eq!(json["peer"]["id"], agent_id);

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = json["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["membership"]["role"], "owner");
    assert_eq!(members[0]["participant"]["id"], f.human_a);
    assert_eq!(members[0]["participant"]["kind"], "human");
    assert_eq!(members[1]["membership"]["role"], "member");
    assert_eq!(members[1]["participant"]["id"], agent_id);
    assert_eq!(members[1]["participant"]["kind"], "agent");

    for path in [
        format!("/v1/rooms/{room_id}"),
        format!("/v1/rooms/{room_id}/members"),
        "/v1/rooms/00000000-0000-0000-0000-000000000000".to_owned(),
        "/v1/rooms/not-a-uuid".to_owned(),
    ] {
        let (status, json, _) = send(
            &f.router,
            Method::GET,
            &path,
            Some(&format!("Bearer {}", f.token_b)),
            &[],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "bob reading {path}");
        assert_eq!(
            json["error"]["code"], "room_not_found",
            "bob reading {path}"
        );
    }
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_b)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["rooms"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn revoke_keeps_room_readable_and_marks_agent_membership_revoked() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;
    let body = serde_json::json!({ "agent_id": agent_id });
    let (status, create_json, _) = post_room(&f.router, &f.token_a, "r-k", body).await;
    assert_eq!(status, StatusCode::CREATED);
    let room_id = create_json["room"]["id"].as_str().unwrap().to_owned();

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
        &format!("/v1/rooms/{room_id}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["room"]["status"], "active",
        "room stays active after revoke"
    );
    assert_eq!(
        json["membership"]["closed_by"],
        serde_json::Value::Null,
        "human membership stays active"
    );
    assert_eq!(json["membership"]["left_at"], serde_json::Value::Null);
    assert_eq!(json["peer"]["status"], "revoked");

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = json["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(
        members[0]["membership"]["closed_by"],
        serde_json::Value::Null
    );
    assert_eq!(members[0]["participant"]["status"], "active");
    assert_eq!(members[1]["membership"]["closed_by"], "agent_revoked");
    assert_eq!(members[1]["participant"]["status"], "revoked");
    assert!(members[1]["membership"]["left_at"].is_string());

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["rooms"].as_array().unwrap().len(), 1);

    let (status, json, _) = post_room(
        &f.router,
        &f.token_a,
        "new-key",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "agent_not_active");

    let (status, json, _) = post_room(
        &f.router,
        &f.token_a,
        "r-k",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["room"]["id"], room_id);
    let memberships = json["memberships"].as_array().unwrap();
    assert_eq!(memberships[0]["closed_by"], serde_json::Value::Null);
    assert_eq!(
        memberships[1]["closed_by"], "agent_revoked",
        "replay reflects current state"
    );
}

#[tokio::test]
async fn abnormal_revoked_human_membership_hides_room_and_404s() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;
    let body = serde_json::json!({ "agent_id": agent_id });
    let (status, create_json, _) = post_room(&f.router, &f.token_a, "r-k", body).await;
    assert_eq!(status, StatusCode::CREATED);
    let room_id = create_json["room"]["id"].as_str().unwrap().to_owned();

    // 构造异常库状态：临时移除 Human 永不撤销守卫后直接撤销 Human
    // owner Membership，验证 HTTP 查询 fail closed。
    sqlx::query("DROP TRIGGER room_memberships_human_close_guard")
        .execute(&f.pool)
        .await
        .unwrap();
    let abnormal = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = 80, left_before_sequence = 0, closed_by = 'removed' \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(&room_id)
    .bind(&f.human_a)
    .execute(&f.pool)
    .await;
    assert!(
        abnormal.is_ok(),
        "abnormal revoked human membership must be constructible after dropping the guard"
    );

    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json["rooms"].as_array().unwrap().is_empty(),
        "revoked human membership must hide the room from list"
    );

    for path in [
        format!("/v1/rooms/{room_id}"),
        format!("/v1/rooms/{room_id}/members"),
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
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(json["error"]["code"], "room_not_found", "{path}");
    }
}

#[tokio::test]
async fn health_and_ready_stay_anonymous() {
    let f = fixture().await;
    let (status, _, _) = send(&f.router, Method::GET, "/health", None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&f.router, Method::GET, "/ready", None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn room_endpoints_use_unified_error_envelope() {
    let f = fixture().await;
    let (status, json, request_id) = send(
        &f.router,
        Method::GET,
        "/v1/rooms/not-a-uuid",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);

    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/rooms/whatever",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json["error"]["code"], "method_not_allowed");
}

#[tokio::test]
async fn revoke_agent_without_room_still_works_and_rooms_empty() {
    let f = fixture().await;
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "a-k").await;
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
        "/v1/rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["rooms"].as_array().unwrap().is_empty());

    let (status, json, _) = post_room(
        &f.router,
        &f.token_a,
        "k",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "agent_not_active");
}

// ===== Human direct / group / invite / remove 契约测试 =====

/// 通过公开 API 创建第二个 Human 的联系人请求并接受（A→B）。
async fn make_accepted_contact(
    f: &Fixture,
    token_a: &str,
    token_b: &str,
    human_a: &str,
    human_b: &str,
) {
    let (status, json, _) = post_contact_request(f, token_a, human_b, "c-key").await;
    assert_eq!(status, StatusCode::CREATED, "contact request: {json}");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/contacts/{human_a}/accept"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "accept: {json}");
    assert_eq!(json["contact"]["status"], "accepted");
}

async fn post_contact_request(
    f: &Fixture,
    token: &str,
    human_id: &str,
    key: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        &f.router,
        Method::POST,
        "/v1/contacts/requests",
        Some(&format!("Bearer {token}")),
        &[("content-type", "application/json"), (KEY_HEADER, key)],
        Some(
            serde_json::json!({ "participant_id": human_id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await
}

#[tokio::test]
async fn human_direct_room_requires_accepted_contact_and_is_exclusive() {
    let f = fixture().await;
    let identity = SqliteIdentityStore::new(f.pool.clone());
    let (human_b, token_b) = add_human(&f.pool, &identity, "Bob").await;

    // 未接受联系人 → 409 contact_not_accepted。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k1")],
        Some(
            serde_json::json!({ "human_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "pending contact blocks: {json}"
    );
    assert_eq!(json["error"]["code"], "contact_not_accepted");

    // 未知 Human → 404 human_not_found。
    let ghost = Uuid::new_v4().to_string();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k2")],
        Some(
            serde_json::json!({ "human_id": ghost })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "human_not_found");

    // 接受联系人后创建成功。
    make_accepted_contact(&f, &f.token_a, &token_b, &f.human_a, &human_b).await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k3")],
        Some(
            serde_json::json!({ "human_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "human direct create: {json}");
    assert_eq!(json["room"]["kind"], "human_direct");
    assert_eq!(json["memberships"].as_array().unwrap().len(), 2);
    let room_id = json["room"]["id"].as_str().unwrap().to_owned();

    // 同 key 同目标重放 → 200 同一 Room。
    let (status, json2, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k3")],
        Some(
            serde_json::json!({ "human_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json2["idempotency_replayed"], true);
    assert_eq!(json2["room"]["id"], room_id);

    // 同一对 Human 另一 key → 409 human_direct_room_exists。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k4")],
        Some(
            serde_json::json!({ "human_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "human_direct_room_exists");

    // B 反向创建 → 409（同一对）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/human-direct-rooms",
        Some(&format!("Bearer {token_b}")),
        &[("content-type", "application/json"), (KEY_HEADER, "hd-k5")],
        Some(
            serde_json::json!({ "human_id": f.human_a })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "human_direct_room_exists");

    // 双向互聊：B 发消息、A 读 History；成员隔离。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token_b}")),
        &[("content-type", "application/json"), (KEY_HEADER, "m-b1")],
        Some(
            serde_json::json!({ "body": "hi from B" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "B sends into human direct: {json}"
    );
    assert_eq!(json["message"]["actor"]["id"], human_b);
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
    assert_eq!(json["messages"][0]["body"], "hi from B");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "m-a1")],
        Some(
            serde_json::json!({ "body": "hi from A" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["message"]["body"], "hi from A");
    assert_eq!(
        json["message"]["actor"]["id"], f.human_a,
        "A's message keeps its own actor"
    );

    // 成员隔离：第三个 Human 读不到。
    let (_human_c, token_c) = add_human(&f.pool, &identity, "Carol").await;
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token_c}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "room_not_found");
}

#[tokio::test]
async fn group_create_invite_remove_and_membership_rules() {
    let f = fixture().await;
    let identity = SqliteIdentityStore::new(f.pool.clone());
    let (human_b, token_b) = add_human(&f.pool, &identity, "Bob").await;
    make_accepted_contact(&f, &f.token_a, &token_b, &f.human_a, &human_b).await;

    // 建群。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "g-k1")],
        Some(
            serde_json::json!({ "name": "项目群" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "group create: {json}");
    assert_eq!(json["room"]["kind"], "group");
    assert_eq!(json["room"]["name"], "项目群");
    assert_eq!(json["memberships"].as_array().unwrap().len(), 1);
    assert_eq!(json["memberships"][0]["role"], "owner");
    let group_id = json["room"]["id"].as_str().unwrap().to_owned();

    // 同 key 同群名重放 → 200 同一 Room；异群名 → 409。
    let (status, json2, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "g-k1")],
        Some(
            serde_json::json!({ "name": "项目群" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json2["room"]["id"], group_id);
    assert_eq!(json2["idempotency_replayed"], true);
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "g-k1")],
        Some(
            serde_json::json!({ "name": "改名" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "idempotency_conflict");

    // 非 owner（B）邀请 → 404 room_not_found。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {token_b}")),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-non-owner"),
        ],
        Some(
            serde_json::json!({ "participant_id": f.human_a })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "non-owner invite: {json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // owner 邀请 accepted Human B → 201。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-b-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite B: {json}");
    assert_eq!(json["membership"]["role"], "member");
    assert_eq!(json["idempotency_replayed"], false);

    // 同 key 重复邀请 → 200 幂等重放，不产生第二条。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-b-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-invite: {json}");
    assert_eq!(json["idempotency_replayed"], true);

    // 新 key 但目标已有 active 周期 → 200（不重复打开周期），key 被记录。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-b-2"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "active target with new key: {json}");
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(json["membership"]["participant_id"], human_b);

    // owner 邀请自己 → 稳定 404 member_not_found（「已有 active 周期
    // 返回 200」不包括 owner 自己）；同 key 重试一致；不写幂等记录。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-self-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": f.human_a })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "owner self invite: {json}");
    assert_eq!(json["error"]["code"], "member_not_found");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-self-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": f.human_a })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "owner self invite retry: {json}"
    );
    assert_eq!(json["error"]["code"], "member_not_found");
    let records: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_member_invite_idempotency \
         WHERE human_id = ? AND key = 'inv-self-1'",
    )
    .bind(f.human_a.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        records, 0,
        "self invite must not write an idempotency record"
    );

    // owner 邀请自己拥有的 active Agent → 201。
    let agent_id = create_agent_via_api(&f.router, &f.token_a, "助手", "g-agent-1").await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-ag-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": agent_id })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite owned agent: {json}");

    // 未接受 Human（Carol）→ 404 member_not_found；他人拥有的 Agent → 404。
    let (human_c, token_c) = add_human(&f.pool, &identity, "Carol").await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-c-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_c })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "member_not_found");
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {token_c}")),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "c-agent-1"),
        ],
        Some(
            serde_json::json!({ "display_name": "Carol's agent" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let carol_agent = json["agent"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-ca-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": carol_agent })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unowned agent invite: {json}"
    );
    assert_eq!(json["error"]["code"], "member_not_found");

    // 邀请进 direct Room → 404 room_not_found（错误 kind）。
    let (status, json, _) = post_room(
        &f.router,
        &f.token_a,
        "direct-for-invite",
        serde_json::json!({ "agent_id": agent_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let direct_id = json["room"]["id"].as_str().unwrap().to_owned();
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{direct_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-direct-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "invite into direct: {json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // 非 owner 移除 → 404；owner 移除 B → 204；B 不能读 History。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{group_id}/members/{human_b}"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "non-owner remove: {json}");
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{group_id}/members/{human_b}"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "owner remove B: {json}");
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{group_id}/messages"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "removed member cannot read: {json}"
    );

    // 移除 owner → 409 owner_not_removable。
    let (status, json, _) = send(
        &f.router,
        Method::DELETE,
        &format!("/v1/rooms/{group_id}/members/{}", f.human_a),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "remove owner: {json}");
    assert_eq!(json["error"]["code"], "owner_not_removable");

    // 已移除成员用新 key 重入 → 201（资格仍满足即可重入，打开新周期）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-b-rejoin"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "re-invite removed: {json}");
    assert_eq!(json["idempotency_replayed"], false);
    assert_eq!(json["membership"]["participant_id"], human_b);

    // 移除前的旧 key 重放只返回历史 closed 周期（200），绝不误开新周期。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "inv-b-1"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "old key replay: {json}");
    assert_eq!(json["idempotency_replayed"], true);
    assert_eq!(json["membership"]["closed_by"], "removed");

    // 群名非法 → 400。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "g-k2")],
        Some(
            serde_json::json!({ "name": "   " })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "blank group name: {json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // group 成员列表：owner 在前。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{group_id}/members"),
        Some(&format!("Bearer {}", f.token_a)),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = json["members"].as_array().unwrap();
    assert_eq!(
        members.len(),
        3,
        "owner + agent active, B revoked (history preserved)"
    );
    assert_eq!(members[0]["membership"]["role"], "owner");
    assert_eq!(members[0]["membership"]["participant_id"], f.human_a);
    let rejoined_b = members
        .iter()
        .find(|m| m["membership"]["participant_id"] == human_b)
        .expect("rejoined B membership listed");
    assert_eq!(
        rejoined_b["membership"]["closed_by"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn invite_rejects_missing_and_invalid_idempotency_keys_without_writes() {
    let f = fixture().await;
    let identity = SqliteIdentityStore::new(f.pool.clone());
    let (human_b, _token_b) = add_human(&f.pool, &identity, "Bob").await;

    // 建群（owner = A）。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "invk-g")],
        Some(
            serde_json::json!({ "name": "契约群" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "group create: {json}");
    let group_id = json["room"]["id"].as_str().unwrap().to_owned();
    let invite_body = serde_json::json!({ "participant_id": human_b })
        .to_string()
        .into_bytes();
    let invite_path = format!("/v1/rooms/{group_id}/members");
    let auth = format!("Bearer {}", f.token_a);

    // 缺失 header → 400 idempotency_key_required。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &invite_path,
        Some(&auth),
        &[("content-type", "application/json")],
        Some(invite_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing key: {json}");
    assert_eq!(json["error"]["code"], "idempotency_key_required");

    // 单值 key 能通过 HTTP 结构检查但格式非法（超长 / 首尾空白 / 非
    // ASCII / 控制字符 tab）：与 transfer 同为 400 invalid_idempotency_key。
    let bad_keys = vec![
        "a".repeat(129),
        " leading-space".to_owned(),
        "trailing-space ".to_owned(),
        "non-ascii-键".to_owned(),
        "ctrl\tkey".to_owned(),
    ];
    for key in &bad_keys {
        let (status, json, _) = send(
            &f.router,
            Method::POST,
            &invite_path,
            Some(&auth),
            &[("content-type", "application/json"), (KEY_HEADER, key)],
            Some(invite_body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "key {key:?}: {json}");
        assert_eq!(
            json["error"]["code"], "invalid_idempotency_key",
            "key {key:?}: {json}"
        );
    }

    // 非法 key 不写任何状态：目标无 Membership 周期，也无幂等记录。
    let memberships: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(group_id.as_str())
    .bind(human_b.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        memberships, 0,
        "invalid keys must not open a membership period"
    );
    let records: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_member_invite_idempotency \
         WHERE human_id = ? AND key = ?",
    )
    .bind(f.human_a.as_str())
    .bind(&bad_keys[0])
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        records, 0,
        "invalid keys must not write an idempotency record"
    );
}

// ===== read cursor / 未读契约测试（CORE_V1_READ_STATE.md）=====

/// 通过公开 API 发送普通 Message，返回 message_id。
async fn send_message_via_api(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    key: &str,
    body: &str,
) -> String {
    let (status, json, _) = send(
        router,
        Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&format!("Bearer {token}")),
        &[("content-type", "application/json"), (KEY_HEADER, key)],
        Some(serde_json::json!({ "body": body }).to_string().into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "send message: {json}");
    json["message"]["id"].as_str().unwrap().to_owned()
}

/// PUT read-cursor 公共 API 助手。
async fn put_read_cursor(
    router: &axum::Router,
    token: &str,
    room_id: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(
        router,
        Method::PUT,
        &format!("/v1/rooms/{room_id}/read-cursor"),
        Some(&format!("Bearer {token}")),
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await
}

/// 建群并邀请 B 的共享前置（A 为 owner）。
async fn setup_group_with_b(f: &Fixture) -> (String, String, String) {
    let identity = SqliteIdentityStore::new(f.pool.clone());
    let (human_b, token_b) = add_human(&f.pool, &identity, "Bob").await;
    make_accepted_contact(f, &f.token_a, &token_b, &f.human_a, &human_b).await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[("content-type", "application/json"), (KEY_HEADER, "rc-g-k")],
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
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "rc-inv-b"),
        ],
        Some(
            serde_json::json!({ "participant_id": human_b })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite B: {json}");
    (group_id, human_b, token_b)
}

/// list / get 的 read_state：初始 null cursor；他人消息计未读、自己消息
/// 不计；PUT 推进 advanced=true、同/旧目标 advanced=false；无新事实。
#[tokio::test]
async fn read_cursor_advance_and_noop_via_public_api() {
    let f = fixture().await;
    let (group_id, _human_b, token_b) = setup_group_with_b(&f).await;

    // B 初始：list 与 get 都是 null cursor、未读 0（A 尚未发消息）。
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["rooms"][0]["read_state"]["last_read_message_id"],
        serde_json::Value::Null
    );
    assert_eq!(json["rooms"][0]["read_state"]["unread_count"], 0);
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{group_id}"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["read_state"]["last_read_message_id"],
        serde_json::Value::Null
    );
    assert_eq!(json["read_state"]["unread_count"], 0);

    // A 发 m1，B 自己发 m2，A 发 m3：B 未读 = {m1, m3} = 2。
    let m1 = send_message_via_api(&f.router, &f.token_a, &group_id, "rc-a-1", "a1").await;
    let _m2 = send_message_via_api(&f.router, &token_b, &group_id, "rc-b-1", "b1").await;
    let m3 = send_message_via_api(&f.router, &f.token_a, &group_id, "rc-a-2", "a2").await;
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        &format!("/v1/rooms/{group_id}"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["read_state"]["unread_count"], 2);

    // 推进到 m1：200 advanced=true，read_state 返回权威值，unread=1（m3）。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": m1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["advanced"], true);
    assert_eq!(json["read_state"]["last_read_message_id"], m1);
    assert_eq!(json["read_state"]["unread_count"], 1);
    assert!(json["request_id"].is_string());

    // 同目标重试：200 advanced=false；不产生新 advancement / Human Event。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": m1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["advanced"], false);
    assert_eq!(json["read_state"]["last_read_message_id"], m1);
    let advancements: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_cursor_advancements")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(advancements, 1, "no-op must not create an advancement");

    // 推进到 m3：advanced=true，unread=0；list 与 get 一致。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": m3 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["advanced"], true);
    assert_eq!(json["read_state"]["unread_count"], 0);
    let (status, json, _) = send(
        &f.router,
        Method::GET,
        "/v1/rooms",
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["rooms"][0]["read_state"]["last_read_message_id"], m3);
    assert_eq!(json["rooms"][0]["read_state"]["unread_count"], 0);

    // 旧目标乱序到达：200 advanced=false，cursor 不倒退。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": m1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["advanced"], false);
    assert_eq!(json["read_state"]["last_read_message_id"], m3);
}

/// PUT read-cursor 错误矩阵：bad UUID / 未知字段 / 非法 JSON / 超限 →
/// 400 invalid_request；未知 / 他 Room / 加入前消息 → 400 invalid_read_cursor；
/// unknown / 非成员 Room → 404 room_not_found；错误不改变 cursor 与事实。
#[tokio::test]
async fn read_cursor_error_matrix_via_public_api() {
    let f = fixture().await;
    let (group_id, _human_b, token_b) = setup_group_with_b(&f).await;

    // B 在群内、A 发一条消息；A 另建第二群（B 不在）。
    let m1 = send_message_via_api(&f.router, &f.token_a, &group_id, "rc-a-1", "a1").await;
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        "/v1/groups",
        Some(&format!("Bearer {}", f.token_a)),
        &[
            ("content-type", "application/json"),
            (KEY_HEADER, "rc-g2-k"),
        ],
        Some(
            serde_json::json!({ "name": "群2" })
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group2 = json["room"]["id"].as_str().unwrap().to_owned();
    let other_room_msg =
        send_message_via_api(&f.router, &f.token_a, &group2, "rc-a-g2-1", "g2-1").await;

    // bad UUID → 400 invalid_request。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": "not-a-uuid" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // 未知字段 / 缺字段 / 非法 JSON → 400 invalid_request。
    for bad_body in [
        serde_json::json!({ "message_id": m1, "sequence": 1 }),
        serde_json::json!({}),
        serde_json::json!({ "message_id": null }),
    ] {
        let (status, json, _) = put_read_cursor(&f.router, &token_b, &group_id, bad_body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert_eq!(json["error"]["code"], "invalid_request");
    }
    let (status, _, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{group_id}/read-cursor"),
        Some(&format!("Bearer {token_b}")),
        &[("content-type", "application/json")],
        Some(b"{ not json".to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 超限（route-local 1 KiB）→ 413 payload_too_large，不影响其他路由。
    let big = format!("\"msg-{}\"", "x".repeat(2048));
    let (status, json, _) = send(
        &f.router,
        Method::PUT,
        &format!("/v1/rooms/{group_id}/read-cursor"),
        Some(&format!("Bearer {token_b}")),
        &[("content-type", "application/json")],
        Some(format!("{{\"message_id\": {big}}}").into_bytes()),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{json}");
    assert_eq!(json["error"]["code"], "payload_too_large");

    // 未知（合法 UUID v4 但不存在）→ 400 invalid_read_cursor。
    let ghost = Uuid::new_v4().to_string();
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": ghost }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_read_cursor");

    // 他 Room 消息 → 400 invalid_read_cursor（统一，不泄露其他 Room 事实）。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": other_room_msg }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_read_cursor");

    // 非成员（B 不在 group2）→ 404 room_not_found。
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group2,
        serde_json::json!({ "message_id": other_room_msg }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // unknown / bad-id Room → 404 room_not_found。
    let ghost_room = Uuid::new_v4().to_string();
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &ghost_room,
        serde_json::json!({ "message_id": m1 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        "bad-room-id",
        serde_json::json!({ "message_id": m1 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");

    // B 退群后推进 → 404；全部错误后 cursor 未动、无 advancement / fact。
    let (status, json, _) = send(
        &f.router,
        Method::POST,
        &format!("/v1/rooms/{group_id}/leave"),
        Some(&format!("Bearer {token_b}")),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");
    let (status, json, _) = put_read_cursor(
        &f.router,
        &token_b,
        &group_id,
        serde_json::json!({ "message_id": m1 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["error"]["code"], "room_not_found");
    let advancements: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_cursor_advancements")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(advancements, 0, "failed PUTs must not create facts");
    let facts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'read.cursor.advanced'",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(facts, 0);
}
