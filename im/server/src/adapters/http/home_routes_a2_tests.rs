//! A2 Home Pairing Transaction HTTP 路由测试：prepare/status/commit/abort
//! 的认证与 redaction 契约、幂等重试、secret 流从不进入响应。

use std::sync::Arc;

use axum::body::Body;
use axum::http::Method;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::home_store::SqliteHomeStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::home;
use crate::domain::human_session::TokenHash;

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    db: MigrationStore,
    /// bootstrap 那位 Human 的 id（邀请用例要断言"新人不等于签发者"）。
    owner_id: String,
    owner: String,
    grant_token: String,
    grant_id: String,
    /// prepare body 提交的 session token SHA-256（hex）。
    session_token_hash: String,
    /// prepare body 提交的 commit token SHA-256（hex）。
    commit_token_hash: String,
    /// status/commit/abort Bearer 呈现的**原始 commit token**（Core 对其
    /// 计算摘要后与存储比对）。
    commit_token: String,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let outcome = crate::application::identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap");
    let owner_token = outcome.token.as_str().to_owned();
    let owner_id = outcome.participant.id.as_str().to_owned();

    let home = SqliteHomeStore::new(pool.clone());
    home::init_home(&home, None).await.expect("init home");

    let issued = home::issue_grant(
        &home,
        &crate::domain::participant::ParticipantId(owner_id.clone()),
        None,
        crate::domain::home::GrantKind::SelfDevice,
        15,
    )
    .await
    .expect("issue grant");
    let grant_token = issued.token.as_str().to_owned();
    let grant_id = issued.grant.grant_id.as_str().to_owned();

    // Host 在 Core 之外预生成的高熵 token；只提交摘要。
    let provisional = "clr_pt_provisional_route_test_token_1";
    let commit_token = "clr_pt_commit_route_test_token_1".to_owned();
    let session_token_hash = sha256_hex(provisional);
    let commit_token_hash = sha256_hex(&commit_token);

    let router = build_router(
        Arc::new(store.clone()),
        Arc::new(identity),
        Arc::new(SqliteAgentStore::new(pool.clone())),
        Arc::new(SqliteRoomStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::message_store::SqliteMessageStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::backend_store::SqliteBackendStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::event_store::SqliteEventStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::contact_store::SqliteContactStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(pool.clone())),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(crate::adapters::sqlite::attachment_store::SqliteAttachmentStore::new(pool.clone())),
        Arc::new(crate::adapters::blob::LocalBlobStorage::new(dir.path().join("media"))),
        Arc::new(home.clone()),
        Arc::new(crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore::new(pool.clone())),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );

    Fixture {
        _dir: dir,
        router,
        db: store,
        owner_id,
        owner: owner_token,
        grant_token,
        grant_id,
        session_token_hash,
        commit_token_hash,
        commit_token,
    }
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(input.as_bytes()))
}

async fn send_json(
    router: &axum::Router,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(path);
    let mut has_body = false;
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let bytes;
    if let Some(body) = body {
        bytes = body.to_string();
        has_body = true;
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    } else {
        bytes = String::new();
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let _ = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    let _ = has_body;
    (status, json)
}

#[tokio::test]
async fn prepare_commit_status_abort_full_flow_with_fresh_secrets() {
    let f = fixture().await;
    let nonce = "host-nonce-1";
    let prepare_body = serde_json::json!({
        "client_nonce": nonce,
        "session_token_hash": f.session_token_hash,
        "commit_token_hash": f.commit_token_hash,
    });

    // prepare：匿名 Grant Bearer；返回非秘密元数据，绝不返回原始 token。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&prepare_body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["status"], "prepared");
    let transaction_id = json["transaction_id"].as_str().unwrap().to_owned();
    let reserved_session_id = json["reserved_session_id"].as_str().unwrap().to_owned();
    let public = json.to_string();
    assert!(
        !public.contains(&f.session_token_hash),
        "no session hash leak"
    );
    assert!(
        !public.contains(&f.commit_token_hash),
        "no commit hash leak"
    );
    assert!(json["home"]["home_id"].as_str().unwrap().len() == 36);
    assert!(json["human"]["human_id"].as_str().is_some());

    // prepare 重试（同一 nonce + 摘要）幂等返回同一事务。
    let (status, json2) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&prepare_body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json2}");
    assert_eq!(json2["transaction_id"], transaction_id);

    // status：凭 commit token Bearer 查询（仅供 Host 恢复）。
    let (status, json3) = send_json(
        &f.router,
        Method::GET,
        &format!("/v1/home/pairing-transactions/{transaction_id}"),
        Some(&f.commit_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json3}");
    assert_eq!(json3["status"], "prepared");

    // commit：凭 commit token Bearer；返回 committed 视图。
    let (status, json4) = send_json(
        &f.router,
        Method::POST,
        &format!("/v1/home/pairing-transactions/{transaction_id}/commit"),
        Some(&f.commit_token),
        Some(&serde_json::json!({"device_label":"work-laptop"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json4}");
    assert_eq!(json4["status"], "committed");
    assert_eq!(json4["reserved_session_id"], reserved_session_id);

    // commit 重试幂等：同一非秘密结果，不产生第二 Session。
    let (status, json5) = send_json(
        &f.router,
        Method::POST,
        &format!("/v1/home/pairing-transactions/{transaction_id}/commit"),
        Some(&f.commit_token),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json5}");
    assert_eq!(json5["transaction_id"], transaction_id);

    // 结果可用：reserved session token hash 已在 human_sessions（权威表）。
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 2, "bootstrap + A2 committed session");

    let _ = f.owner;
    let _ = f.grant_id;
}

#[tokio::test]
async fn prepare_with_missing_or_invalid_grant_is_redacted() {
    let f = fixture().await;
    let body = serde_json::json!({
        "client_nonce": "nonce-x",
        "session_token_hash": "a".repeat(64),
        "commit_token_hash": "b".repeat(64),
    });
    // 缺 Bearer → authentication_required。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        None,
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");
    // 错误格式 Bearer → grant_invalid（匿名兑换语义）。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some("not-a-grant-token"),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "grant_invalid");
    // 未知但格式合法的 Grant → grant_invalid（不泄露存在性）。
    let unknown = crate::domain::grant_token::GrantToken::generate().unwrap();
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(unknown.as_str()),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "grant_invalid");
}

#[tokio::test]
async fn prepare_with_bad_digest_or_nonce_shape_is_invalid_request() {
    let f = fixture().await;
    // 非 64-hex 摘要 → 400 invalid_request。
    let z64 = "z".repeat(64);
    let a63 = "a".repeat(63);
    for bad in ["abc", "", z64.as_str(), a63.as_str()] {
        let body = serde_json::json!({
            "client_nonce": "nonce-bad",
            "session_token_hash": bad,
            "commit_token_hash": "b".repeat(64),
        });
        let (status, json) = send_json(
            &f.router,
            Method::POST,
            "/v1/home/pairing-grants/prepare",
            Some(&f.grant_token),
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(json["error"]["code"], "invalid_request", "{bad}");
    }
    // 空 nonce → invalid_request。
    let body = serde_json::json!({
        "client_nonce": "",
        "session_token_hash": "a".repeat(64),
        "commit_token_hash": "b".repeat(64),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn status_commit_abort_require_commit_token_and_redact_unknown() {
    let f = fixture().await;
    let body = serde_json::json!({
        "client_nonce": "nonce-auth",
        "session_token_hash": sha256_hex("clr_pt_prov_auth"),
        "commit_token_hash": sha256_hex("clr_pt_cmt_auth"),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let transaction_id = json["transaction_id"].as_str().unwrap().to_owned();

    let path_status = format!("/v1/home/pairing-transactions/{transaction_id}");
    // 缺 commit token Bearer → authentication_required（status 是恢复路径，仍鉴权）。
    let (status, json) = send_json(&f.router, Method::GET, &path_status, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_required");
    // 错误 commit token（任意原始 token 而非授权 token）→
    // pairing_transaction_invalid（不泄露存在性）。
    let (status, json) = send_json(
        &f.router,
        Method::GET,
        &path_status,
        Some("clr_pt_cmt_wrong"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");

    // commit 用错误 token → pairing_transaction_invalid。
    let commit_path = format!("/v1/home/pairing-transactions/{transaction_id}/commit");
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        &commit_path,
        Some("clr_pt_cmt_wrong"),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");

    // abort 错误 token → pairing_transaction_invalid。
    let (status, json) = send_json(
        &f.router,
        Method::DELETE,
        &path_status,
        Some("clr_pt_cmt_wrong"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");

    // 未知 transaction id → pairing_transaction_invalid（bad-id 与不存在同一错误面）。
    let (status, json) = send_json(
        &f.router,
        Method::GET,
        "/v1/home/pairing-transactions/not-a-uuid",
        Some("clr_pt_cmt_auth"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");
}

#[tokio::test]
async fn abort_abandoned_prepared_then_commit_fails_and_grant_stays_active() {
    let f = fixture().await;
    let body = serde_json::json!({
        "client_nonce": "nonce-abort",
        "session_token_hash": sha256_hex("clr_pt_prov_abort"),
        "commit_token_hash": sha256_hex("clr_pt_cmt_abort"),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let transaction_id = json["transaction_id"].as_str().unwrap().to_owned();

    // abort（凭 commit token）。
    let path = format!("/v1/home/pairing-transactions/{transaction_id}");
    let (status, _) = send_json(
        &f.router,
        Method::DELETE,
        &path,
        Some("clr_pt_cmt_abort"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 已 aborted：commit 仍失败；重复 abort 对同一资源幂等成功。
    let commit_path = format!("{path}/commit");
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        &commit_path,
        Some("clr_pt_cmt_abort"),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");
    let (status, _) = send_json(
        &f.router,
        Method::DELETE,
        &path,
        Some("clr_pt_cmt_abort"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Grant 未被 consume/revoke：用同一 grant 重新签发 prepare 仍可行（新 secrets）。
    let body2 = serde_json::json!({
        "client_nonce": "nonce-abort-2",
        "session_token_hash": sha256_hex("clr_pt_prov_abort_2"),
        "commit_token_hash": sha256_hex("clr_pt_cmt_abort_2"),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&body2),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
}

#[tokio::test]
async fn commit_activates_a_usable_session_and_secret_hashes_never_returned() {
    let f = fixture().await;
    let body = serde_json::json!({
        "client_nonce": "nonce-use",
        "session_token_hash": sha256_hex("clr_pt_prov_use"),
        "commit_token_hash": sha256_hex("clr_pt_cmt_use"),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let transaction_id = json["transaction_id"].as_str().unwrap().to_owned();
    let reserved = json["reserved_session_id"].as_str().unwrap().to_owned();

    // commit 后返回 committed；不发任何 token/hash。
    let commit_path = format!("/v1/home/pairing-transactions/{transaction_id}/commit");
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        &commit_path,
        Some("clr_pt_cmt_use"),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let response = json.to_string();
    assert!(
        !response.contains(sha256_hex("clr_pt_prov_use").as_str()),
        "no session hash"
    );
    assert!(
        !response.contains(sha256_hex("clr_pt_cmt_use").as_str()),
        "no commit hash"
    );

    // 权威事实：session_metadata 指向该 reserved session，device label 可读、
    // 无 token。
    let (device_label, session_id): (Option<String>, String) = sqlx::query_as(
        "SELECT m.device_label, s.id FROM session_metadata m \
         JOIN human_sessions s ON s.id = m.session_id",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(session_id, reserved);
    assert!(device_label.is_none());
}

#[tokio::test]
async fn human_session_token_cannot_authorize_any_a2_endpoint() {
    let f = fixture().await;
    // Human Session token 不能充当 Grant（exchange 语义），也不能充当
    // commit token：prepare 对 Human Session 一律 grant_invalid。
    let body = serde_json::json!({
        "client_nonce": "nonce-x",
        "session_token_hash": "a".repeat(64),
        "commit_token_hash": "b".repeat(64),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.owner),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "grant_invalid");

    // status 用 Human Session token 也统一 pairing_transaction_invalid。
    let _ = TokenHash::parse_hex(&"a".repeat(64));
    let (status, json) = send_json(
        &f.router,
        Method::GET,
        "/v1/home/pairing-transactions/00000000-0000-4000-8000-000000000000",
        Some(&f.owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");
}

// 资源绑定回归：commit token 是资源（transaction）绑定凭证，不是仅凭摘要
// 寻址。把真实事务的 commit token 用在**另一个**合法 UUID 事务路径上必须
// 返回 pairing_transaction_invalid，且不得改变真实事务状态。
#[tokio::test]
async fn real_commit_token_on_unrelated_transaction_path_is_rejected_and_preserves_real() {
    let f = fixture().await;
    let prepare_body = serde_json::json!({
        "client_nonce": "nonce-binding",
        "session_token_hash": sha256_hex("clr_pt_prov_binding"),
        "commit_token_hash": sha256_hex("clr_pt_cmt_binding"),
    });
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&prepare_body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let real_transaction_id = json["transaction_id"].as_str().unwrap().to_owned();
    assert_eq!(json["status"], "prepared");

    // 同一 commit token 作用在另一个合法 UUID v4 transaction path 上。
    let unrelated = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        &format!("/v1/home/pairing-transactions/{unrelated}/commit"),
        Some("clr_pt_cmt_binding"),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "pairing_transaction_invalid");

    // 真实事务未被触动：仍 prepared，且真实 commit token 依然有效。
    let (status, json) = send_json(
        &f.router,
        Method::GET,
        &format!("/v1/home/pairing-transactions/{real_transaction_id}"),
        Some("clr_pt_cmt_binding"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["status"], "prepared");
    assert_eq!(json["transaction_id"], real_transaction_id);
}

#[tokio::test]
async fn malformed_commit_body_is_rejected_without_committing() {
    let f = fixture().await;
    let body = serde_json::json!({
        "client_nonce": "nonce-malformed-commit",
        "session_token_hash": sha256_hex("clr_pt_prov_malformed"),
        "commit_token_hash": sha256_hex("clr_pt_cmt_malformed"),
    });
    let (status, prepared) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&f.grant_token),
        Some(&body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let transaction_id = prepared["transaction_id"].as_str().unwrap();
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v1/home/pairing-transactions/{transaction_id}/commit"
        ))
        .header(header::AUTHORIZATION, "Bearer clr_pt_cmt_malformed")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{"))
        .unwrap();
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");

    let (status, current) = send_json(
        &f.router,
        Method::GET,
        &format!("/v1/home/pairing-transactions/{transaction_id}"),
        Some("clr_pt_cmt_malformed"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["status"], "prepared");
}

// ---------- 邀请一个人进这个世界（grants = new_human） ----------

/// 邀请的完整闭环：认证 Human 签发 → 匿名兑换（自己命名）→ 新身份真的能用，
/// 且邀请者名下多出一条**待答**关系申请（事后同意，不是预先同意）。
#[tokio::test]
async fn invite_over_http_creates_the_invitee_and_a_pending_contact_request() {
    let f = fixture().await;

    // 1) 认证 Human 签发一张邀请码（老客户端不带 grants ⇒ 仍是设备配对）。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants",
        Some(&f.owner),
        Some(&serde_json::json!({ "grants": "new_human" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["grants"], "new_human");
    let invite_token = json["token"].as_str().unwrap().to_owned();
    let invite_grant_id = json["grant_id"].as_str().unwrap().to_owned();

    // 2) 匿名兑换：被邀请者**自己**给名字。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&invite_token),
        Some(&serde_json::json!({ "display_name": "阿元" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let newcomer_token = json["token"].as_str().unwrap().to_owned();
    let newcomer_id = json["session"]["human_id"].as_str().unwrap().to_owned();
    assert_ne!(
        newcomer_id,
        f.owner_id.as_str(),
        "兑换出来的是**新**主体，不是签发者"
    );

    // 3) 新身份真的能用（拿它读自己）。
    let (status, me) = send_json(
        &f.router,
        Method::GET,
        "/v1/me",
        Some(&newcomer_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["participant"]["id"], newcomer_id);
    assert_eq!(me["participant"]["display_name"], "阿元");

    // 4) 邀请者名下多出一条**待答**关系申请（方向是 outgoing：等对方回答）。
    let (status, contacts) =
        send_json(&f.router, Method::GET, "/v1/contacts", Some(&f.owner), None).await;
    assert_eq!(status, StatusCode::OK, "{contacts}");
    let row = contacts["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["peer"]["id"] == newcomer_id.as_str())
        .unwrap_or_else(|| panic!("invite should leave a pending contact request: {contacts}"));
    assert_eq!(row["status"], "pending");
    assert_eq!(row["direction"], "outgoing");

    // 5) 码是一次性的：重放统一 grant_invalid，且不再造人。
    let (status, replay) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&invite_token),
        Some(&serde_json::json!({ "display_name": "另一个人" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{replay}");
    assert_eq!(replay["error"]["code"], "grant_invalid");

    // 6) Grant 已消费 ⇒ 签发者那边看不到它（既有口径）。
    let (status, list) = send_json(
        &f.router,
        Method::GET,
        "/v1/home/pairing-grants",
        Some(&f.owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !list["grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["grant_id"] == invite_grant_id.as_str()),
        "consumed grant must not be listed: {list}"
    );
}

/// 邀请码**必须**由被邀请者自己命名：没给名字 ⇒ 400 且**不消费**（补上即可）。
#[tokio::test]
async fn invite_exchange_without_a_name_is_400_and_the_code_still_works() {
    let f = fixture().await;
    let (_, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants",
        Some(&f.owner),
        Some(&serde_json::json!({ "grants": "new_human" })),
    )
    .await;
    let invite_token = json["token"].as_str().unwrap().to_owned();

    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&invite_token),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // 补上名字 ⇒ 同一张码成功（证明上一次失败没有消费它）。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&invite_token),
        Some(&serde_json::json!({ "display_name": "阿元" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
}

/// 用途与请求体必须对上：未知 `grants` 值、设备配对带名字，都是 400。
#[tokio::test]
async fn grant_kind_and_body_shape_must_agree() {
    let f = fixture().await;

    // 未知用途：封闭词表，不猜、不静默降级。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants",
        Some(&f.owner),
        Some(&serde_json::json!({ "grants": "invite_everyone" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // 设备配对那一类不接受名字（f.grant_token 就是 fixture 里那张设备码）。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&f.grant_token),
        Some(&serde_json::json!({ "display_name": "阿元" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error"]["code"], "invalid_request");

    // 而它按老样子兑换仍然成功，且会话归签发者本人（既有语义一字未动）。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&f.grant_token),
        Some(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["session"]["human_id"], f.owner_id.as_str());
}

/// 邀请码**不走两阶段配对路径**（本切片明确不做）：那条路径是设备配对的
/// 事务化形态，事务表里没有"被邀请者自己给的名字"这一格。这里必须 fail-closed
/// ——绝不静默按设备配对给别人铸一支会话。
#[tokio::test]
async fn invite_grant_is_refused_by_the_two_phase_path() {
    let f = fixture().await;
    let (_, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants",
        Some(&f.owner),
        Some(&serde_json::json!({ "grants": "new_human" })),
    )
    .await;
    let invite_token = json["token"].as_str().unwrap().to_owned();

    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/prepare",
        Some(&invite_token),
        Some(&serde_json::json!({
            "client_nonce": "host-nonce-invite",
            "session_token_hash": f.session_token_hash,
            "commit_token_hash": f.commit_token_hash,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    // 用**既有**的统一码（不新增错误码）：在那条路径上这张码就是"不可用"。
    assert_eq!(json["error"]["code"], "grant_invalid");

    // 而简单兑换仍然可用（拒绝的是"两阶段这条路"，不是这张码）。
    let (status, json) = send_json(
        &f.router,
        Method::POST,
        "/v1/home/pairing-grants/exchange",
        Some(&invite_token),
        Some(&serde_json::json!({ "display_name": "阿元" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
}
