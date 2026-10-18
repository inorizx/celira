//! **测试夹具专用**的关系种子（批次三·第三刀 第③步起）。
//!
//! 第③步把准入判据从 ownership 换成"**双方确认过的关系**"（accepted 关系段）。
//! 测试套件的通用夹具原本是"Owner 跟自己的 Agent 开一间私人房"——那是旧判据的
//! 形状；新判据下，所有"建房 / 拉群"的夹具都必须**先有一段 accepted 关系**。
//!
//! ⚠ 这是**夹具**（直接落 schema），不是产品路径：产品路径是"申请 → 对方接受"
//! （`POST /v1/contacts/requests` + `POST /v1/contacts/{id}/accept`，见
//! `backend_contact_routes_tests` / `backend_contact_blackbox`）。用它，只是因为
//! 绝大多数用例考的不是"关系怎么成立"，而是**关系成立之后**的那些事实
//! （消息、撤回、事件、投递、呈现……）——让它们各自去走一遍关系流程，只会把
//! 每个用例变成关系用例的重复，而不会多证明任何东西。
//!
//! 判据的**正反两面**由专门的关系用例守：`contact_reversibility_migration_tests`
//! 与 `backend_contact_*` 里既有"accepted 够用"，也有"pending 不够用"。

use sqlx::SqlitePool;

use crate::domain::human_session::TokenHash;

/// 与 [`seed_accepted_relation`] 同一件事，但**只给 Human 的 token**——HTTP 侧的夹具
/// 大多只留 token、不留 Human id，而会话表里记着 token 属于谁。
///
/// token 只用于**查身份**（`human_sessions.token_hash`），不落盘、不进日志。
pub(crate) async fn seed_accepted_relation_for_token(
    pool: &SqlitePool,
    human_token: &str,
    peer_id: &str,
) {
    let hash = TokenHash::from_token_str(human_token).hex();
    let human_id: Option<String> =
        sqlx::query_scalar("SELECT human_id FROM human_sessions WHERE token_hash = ?")
            .bind(hash)
            .fetch_optional(pool)
            .await
            .expect("look up human by token");
    let Some(human_id) = human_id else {
        panic!("fixture token does not belong to any session");
    };
    seed_accepted_relation(pool, &human_id, peer_id).await;
}

/// 在两主体之间种一段 **accepted** 的 active 关系段（方向：`a` 发起、`b` 接受）。
///
/// 幂等：该对已经有 active 段时直接返回（不产生第二条——表本身也只允许一条）。
pub(crate) async fn seed_accepted_relation(pool: &SqlitePool, a: &str, b: &str) {
    let (a_id, b_id) = if a < b { (a, b) } else { (b, a) };
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM contact_periods WHERE a_id = ? AND b_id = ? AND active_marker = 1 LIMIT 1",
    )
    .bind(a_id)
    .bind(b_id)
    .fetch_optional(pool)
    .await
    .expect("probe existing relation");
    if existing.is_some() {
        return;
    }
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, \
          closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, 'accepted', 0, 0, NULL, NULL, 1, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(a_id)
    .bind(b_id)
    .bind(a_id)
    .bind(b_id)
    .bind(format!("fixture-relation-{}", uuid::Uuid::new_v4()))
    .execute(pool)
    .await
    .expect("seed accepted relation");
}
