//! SQLite Contact adapter 测试：真实临时数据库上的请求创建、反向收敛、
//! 幂等、自我请求拒绝、目标校验、接受权限与终态、数据库守卫。

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::application::contact as contact_application;
use crate::application::identity::bootstrap_human;
use crate::domain::contact::{ContactStatus, canonical_pair};
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::identity::{IdentityStore as _, NewSession};

struct Fixture {
    _dir: tempfile::TempDir,
    identity: SqliteIdentityStore,
    contact: SqliteContactStore,
    pool: SqlitePool,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let pool = store.pool().clone();
    Fixture {
        _dir: dir,
        identity: SqliteIdentityStore::new(pool.clone()),
        contact: SqliteContactStore::new(pool.clone()),
        pool,
    }
}

/// 通过 schema 直接创建第二个 Human（并补发 Session），返回 Participant。
async fn add_human(
    pool: &SqlitePool,
    identity: &SqliteIdentityStore,
    display_name: &str,
) -> Participant {
    let id = ParticipantId(Uuid::new_v4().to_string());
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', ?, 0, NULL)",
    )
    .bind(id.as_str())
    .bind(display_name)
    .execute(pool)
    .await
    .expect("insert human");
    let token = OpaqueToken::generate().expect("token");
    let session = HumanSession::new(
        SessionId(Uuid::new_v4().to_string()),
        id.clone(),
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
        .expect("issue session");
    Participant::new_human(
        id,
        DisplayName::parse(display_name).unwrap(),
        UtcMillis::now(),
    )
}

#[tokio::test]
async fn request_create_reverse_convergence_and_idempotency() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, &f.identity, "Bob").await;
    let carol = add_human(&f.pool, &f.identity, "Carol").await;

    // A→B 首次创建 201（Created）。
    let outcome =
        contact_application::send_contact_request(&f.contact, &alice, bob.id.as_str(), "k-ab")
            .await
            .expect("request must succeed");
    let contact_application::SendContactRequestOutcome::Created { contact, peer } = outcome else {
        panic!("first request must be Created");
    };
    assert_eq!(contact.status, ContactStatus::Pending);
    assert_eq!(contact.requester_id, alice.id);
    assert_eq!(contact.addressee_id, bob.id);
    assert_eq!(peer.id, bob.id);
    assert_eq!(peer.display_name.as_str(), "Bob");

    // 同 key 幂等重放 → Existing，不创建第二条。
    let outcome =
        contact_application::send_contact_request(&f.contact, &alice, bob.id.as_str(), "k-ab")
            .await
            .expect("replay");
    assert!(matches!(
        outcome,
        contact_application::SendContactRequestOutcome::Existing { .. }
    ));
    assert_eq!(count_contacts(&f).await, 1);

    // 不同 key 重复请求 → Existing（同一对关系）。
    let outcome =
        contact_application::send_contact_request(&f.contact, &alice, bob.id.as_str(), "k-ab-2")
            .await
            .expect("repeat");
    assert!(matches!(
        outcome,
        contact_application::SendContactRequestOutcome::Existing { .. }
    ));

    // B→A 反向请求 → Existing 同一 pending 关系，不改变 requester/addressee。
    let outcome =
        contact_application::send_contact_request(&f.contact, &bob, alice.id.as_str(), "k-ba")
            .await
            .expect("reverse");
    let contact_application::SendContactRequestOutcome::Existing { contact, .. } = outcome else {
        panic!("reverse request must return the existing relation");
    };
    assert_eq!(contact.requester_id, alice.id, "requester must not flip");
    assert_eq!(contact.addressee_id, bob.id, "addressee must not flip");
    assert_eq!(contact.status, ContactStatus::Pending);
    assert_eq!(count_contacts(&f).await, 1, "one symmetric fact per pair");

    // 同 requester + key 换目标 → 409 冲突。
    let err =
        contact_application::send_contact_request(&f.contact, &alice, carol.id.as_str(), "k-ab")
            .await
            .expect_err("key reuse with different target must conflict");
    assert!(matches!(
        err,
        contact_application::SendContactRequestError::IdempotencyConflict
    ));

    // 自我请求 → 400。
    let err =
        contact_application::send_contact_request(&f.contact, &alice, alice.id.as_str(), "k-self")
            .await
            .expect_err("self request must be rejected");
    assert!(matches!(
        err,
        contact_application::SendContactRequestError::InvalidSelfRequest
    ));

    // 未知目标 → 404；Agent 目标 → 404。
    let ghost = Uuid::new_v4().to_string();
    let err = contact_application::send_contact_request(&f.contact, &alice, &ghost, "k-ghost")
        .await
        .expect_err("unknown target must be rejected");
    assert!(matches!(
        err,
        contact_application::SendContactRequestError::NotFound
    ));
    assert_eq!(count_contacts(&f).await, 1);
}

#[tokio::test]
async fn accept_is_addressee_only_and_moves_pending_to_accepted_once() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, &f.identity, "Bob").await;
    contact_application::send_contact_request(&f.contact, &alice, bob.id.as_str(), "k-ab")
        .await
        .expect("request");

    // requester 不能接受自己的请求。
    let err = contact_application::accept_contact(&f.contact, &alice.id, bob.id.as_str())
        .await
        .expect_err("requester cannot accept");
    assert!(matches!(
        err,
        contact_application::ContactQueryError::NotFound
    ));

    // addressee 接受成功 → accepted，accepted_at 必有。
    let (contact, peer) =
        contact_application::accept_contact(&f.contact, &bob.id, alice.id.as_str())
            .await
            .expect("addressee accept");
    assert_eq!(contact.status, ContactStatus::Accepted);
    assert!(contact.accepted_at.is_some());
    assert_eq!(peer.id, alice.id);

    // 重复接受幂等：仍 accepted，accepted_at 不变。
    let (contact2, _) = contact_application::accept_contact(&f.contact, &bob.id, alice.id.as_str())
        .await
        .expect("re-accept");
    assert_eq!(contact2.status, ContactStatus::Accepted);
    assert_eq!(contact2.accepted_at, contact.accepted_at);

    // 无关系对 → 404。
    let carol = add_human(&f.pool, &f.identity, "Carol").await;
    let err = contact_application::accept_contact(&f.contact, &bob.id, carol.id.as_str())
        .await
        .expect_err("no relation must be rejected");
    assert!(matches!(
        err,
        contact_application::ContactQueryError::NotFound
    ));

    // 数据库守卫：accepted 段不可改回 pending；accepted_at 不可改写。
    let (a, b) = canonical_pair(&alice.id, &bob.id);
    let res = sqlx::query(
        "UPDATE contact_periods SET state = 'pending', accepted_at = NULL \
         WHERE a_id = ? AND b_id = ?",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .execute(&f.pool)
    .await;
    assert!(res.is_err(), "accepted must be terminal");
    let res = sqlx::query(
        "UPDATE contact_periods SET accepted_at = accepted_at + 1 \
         WHERE a_id = ? AND b_id = ?",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .execute(&f.pool)
    .await;
    assert!(res.is_err(), "accepted_at must be immutable");
}

#[tokio::test]
async fn list_contacts_returns_both_sides_and_states() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, &f.identity, "Bob").await;
    let carol = add_human(&f.pool, &f.identity, "Carol").await;

    contact_application::send_contact_request(&f.contact, &alice, bob.id.as_str(), "k-ab")
        .await
        .expect("ab request");
    contact_application::send_contact_request(&f.contact, &carol, alice.id.as_str(), "k-ca")
        .await
        .expect("ca request");

    let alice_contacts = contact_application::list_contacts(&f.contact, &alice.id)
        .await
        .expect("alice list");
    assert_eq!(alice_contacts.len(), 2);
    let pending = alice_contacts
        .iter()
        .find(|item| item.peer.id == bob.id)
        .expect("bob as peer");
    assert_eq!(pending.contact.status, ContactStatus::Pending);
    let incoming = alice_contacts
        .iter()
        .find(|item| item.peer.id == carol.id)
        .expect("carol as peer");
    assert_eq!(incoming.contact.status, ContactStatus::Pending);
    assert_eq!(incoming.contact.requester_id, carol.id);

    // 接受后状态原样公开。
    contact_application::accept_contact(&f.contact, &bob.id, alice.id.as_str())
        .await
        .expect("accept");
    let alice_contacts = contact_application::list_contacts(&f.contact, &alice.id)
        .await
        .expect("alice list after accept");
    let accepted = alice_contacts
        .iter()
        .find(|item| item.peer.id == bob.id)
        .unwrap();
    assert_eq!(accepted.contact.status, ContactStatus::Accepted);

    // 联系人公开视图不泄露幂等键：协议视图没有该字段。
    let view_json = serde_json::to_value(crate::protocol::contact::ContactView {
        peer: crate::protocol::room::ParticipantMinView {
            id: "p".into(),
            kind: "human".into(),
            display_name: "d".into(),
            status: "active".into(),
        },
        status: "accepted".into(),
        direction: "incoming".into(),
        created_at: "t".into(),
        accepted_at: None,
    })
    .unwrap();
    assert!(
        view_json.get("idempotency").is_none() && view_json.get("idempotency_key").is_none(),
        "public contact view must not leak idempotency keys"
    );
}

async fn count_contacts(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM contact_periods")
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn db_guards_reject_invalid_contact_rows() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, &f.identity, "Bob").await;
    let (a, b) = canonical_pair(&alice.id, &bob.id);

    // pending 段不能带 accepted_at。
    let res = sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-000000000001', ?, ?, ?, ?, 'pending', 1, 2, 'k1')",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(a.as_str())
    .bind(b.as_str())
    .execute(&f.pool)
    .await;
    assert!(res.is_err(), "pending with accepted_at must be rejected");

    // accepted 段必须带 accepted_at。
    let res = sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-000000000002', ?, ?, ?, ?, 'accepted', 1, NULL, 'k2')",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(a.as_str())
    .bind(b.as_str())
    .execute(&f.pool)
    .await;
    assert!(
        res.is_err(),
        "accepted without accepted_at must be rejected"
    );

    // 关系段不可硬删除。
    contact_application::send_contact_request(&f.contact, &alice, bob.id.as_str(), "k-ab")
        .await
        .expect("request");
    let res = sqlx::query("DELETE FROM contact_periods")
        .execute(&f.pool)
        .await;
    assert!(res.is_err(), "contact periods cannot be deleted");
}

#[tokio::test]
async fn cancelled_send_contact_request_rolls_back_before_connection_reuse() {
    let f = fixture().await;
    // 单连接池保证 abort 后的查询和下一次请求必须复用同一物理连接；
    // 如果 Transaction Drop 没有在归池前回滚，后续写会撞上遗留的
    // BEGIN IMMEDIATE 写锁或嵌套事务。
    let options = SqliteConnectOptions::new()
        .filename(f._dir.path().join("core.db"))
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let cancel_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open single-connection cancellation pool");
    let cancel_identity = SqliteIdentityStore::new(cancel_pool.clone());
    let cancel_contact = SqliteContactStore::new(cancel_pool.clone());
    let next_contact = cancel_contact.clone();
    let alice = bootstrap_human(&cancel_identity, "Alice")
        .await
        .unwrap()
        .participant;
    let alice_next = alice.clone();
    let bob = add_human(&cancel_pool, &cancel_identity, "Bob").await;
    let bob_next = bob.clone();

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (released_tx, released_rx) = tokio::sync::oneshot::channel();
    cancel_contact.arm_commit_checkpoint(reached_tx, released_rx);

    // send_contact_request 写事务在「pending 行已写入、commit 前」挂起，
    // 等待测试 abort。
    let handle = tokio::spawn(async move {
        contact_application::send_contact_request(
            &cancel_contact,
            &alice,
            bob.id.as_str(),
            "cancel-key",
        )
        .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
        .await
        .expect("contact request must reach the commit checkpoint in time")
        .expect("contact request must not fail before the checkpoint");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());
    drop(released_tx);

    let contacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_periods")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(contacts, 0, "cancelled request must leave no contact row");

    let outcome = contact_application::send_contact_request(
        &next_contact,
        &alice_next,
        bob_next.id.as_str(),
        "cancel-key",
    )
    .await
    .expect("next request on the same single connection must succeed");
    let contact_application::SendContactRequestOutcome::Created { .. } = outcome else {
        panic!("contact request after cancellation must be Created, not Existing");
    };
    assert_eq!(count_contacts(&f).await, 1);
}
