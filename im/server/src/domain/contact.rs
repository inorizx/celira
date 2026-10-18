//! Contact：Human–Human 对称关系段。
//!
//! 契约（CORE_V1_CONTACT_REVERSIBILITY.md）：关系是一条 Human–Human 公共
//! 关系，不是 Room Membership；以可结束的周期表达——同一对 Human 至多一个
//! active 段（规范 (a_id, b_id) 配对），关系查询对称；A→B 与 B→A 使用同一
//! 段。最小状态 pending → accepted；拒绝（pending → declined）与解除
//! （accepted → ended）都是关闭周期（追加新事实、不改写历史）。pending 请求
//! 不能创建 Human direct Room，也不能邀请进 group；闭合段不进入公开联系人
//! 视图。

use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;

/// 关系段状态。accepted 不是终态：任一方可以解除（关闭段）。公开视图的
/// status 只会出现 pending / accepted（闭合段不列出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactStatus {
    Pending,
    Accepted,
}

impl ContactStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
        }
    }

    /// 数据库枚举值 → 领域类型；未知值返回 None。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            _ => None,
        }
    }
}

/// 一条已持久化的联系人关系公共事实。
///
/// `a_id` / `b_id` 是规范顺序的配对两端（a_id < b_id），保证同一对 Human
/// 只有一条关系；`requester_id` / `addressee_id` 表达发起方向，只有
/// addressee 可以接受 pending 请求。accepted 时 `accepted_at` 必有。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub a_id: ParticipantId,
    pub b_id: ParticipantId,
    pub requester_id: ParticipantId,
    pub addressee_id: ParticipantId,
    pub status: ContactStatus,
    pub created_at: UtcMillis,
    pub accepted_at: Option<UtcMillis>,
}

/// 计算两个 Participant ID 的规范配对（a_id < b_id），用于「同一对 Human
/// 只有一条关系」的唯一裁决。字符串比较对 UUID 是确定性的。
pub fn canonical_pair(a: &ParticipantId, b: &ParticipantId) -> (ParticipantId, ParticipantId) {
    if a.as_str() < b.as_str() {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_rejects_unknown() {
        assert_eq!(ContactStatus::Pending.as_str(), "pending");
        assert_eq!(ContactStatus::Accepted.as_str(), "accepted");
        assert_eq!(
            ContactStatus::parse("pending"),
            Some(ContactStatus::Pending)
        );
        assert_eq!(
            ContactStatus::parse("accepted"),
            Some(ContactStatus::Accepted)
        );
        for unknown in ["", "blocked", "Pending"] {
            assert_eq!(ContactStatus::parse(unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn canonical_pair_is_deterministic_and_order_independent() {
        let a = ParticipantId("11111111-1111-4111-8111-111111111111".into());
        let b = ParticipantId("22222222-2222-4222-8222-222222222222".into());
        let (x, y) = canonical_pair(&a, &b);
        let (y2, x2) = canonical_pair(&b, &a);
        assert_eq!(x, y2);
        assert_eq!(y, x2);
        assert_eq!(x, a);
        assert_eq!(y, b);
        assert!(x.as_str() < y.as_str());
    }
}
