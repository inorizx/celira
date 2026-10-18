//! 主体进行态用例：可见性裁决 + 进程内易失读写编排。
//!
//! 权威契约：`docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md` §6。
//!
//! 本层组合三个窄端口，仅此而已：
//! - `RoomStore::get_room_members`（**既有**成员读取用例：一次调用同时给出
//!   "viewer 是不是该房 active 成员"与成员名单——可见性裁决与成员投影都
//!   复用既有实现，不发明第二套权限）；
//! - `LivePresence`（进程内易失：`last_seen` / `typing`）；
//! - `ConnectionPresence` ×2（Human 的 WS 订阅表、Agent 的门铃门闩）。
//!
//! **在线判据**（不新增写者）：
//! - Human：手里有活着的 Human Event WS 连接；
//! - Agent：挂着门铃 SSE **或**最近 `SEEN_TTL` 内被看见过（拉取式 Backend
//!   的兜底——"被看见"是认证路径的副产品，不是心跳上报）。
//!
//! **零执行面**：这里没有阶段、生成流、revision、草稿正文——那些随旧
//! Live State 于 2026-09-28 删除，且不打算回来。

use crate::application::room::{RoomQueryError, get_room_members};
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::live::{ConnectionPresence, LivePresence};
use crate::ports::room::RoomStore;

/// 该房一位成员的进行态投影（人机同形——`kind` 只用于呈现，不改变判据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberLive {
    pub participant_id: ParticipantId,
    pub display_name: DisplayName,
    pub kind: ParticipantKind,
    /// 此刻是否在线（连接派生）。
    pub online: bool,
    /// 此刻是否正在这间房写（易失，`TYPING_TTL` 内有效）。
    pub typing: bool,
}

/// 一间房的进行态快照：该房**全部 active 成员**（人 + Agent）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomLiveSnapshot {
    pub room_id: RoomId,
    pub members: Vec<MemberLive>,
}

/// 读取该房进行态快照。
///
/// 可见性：viewer 必须是该房 active 成员（复用 `get_room_members` 的既有
/// 裁决）；unknown / bad-id / 非成员统一 `NotFound`（对外 `404
/// room_not_found`，不泄露房间与他人是否存在）。
pub async fn room_live_snapshot(
    room: &dyn RoomStore,
    live: &dyn LivePresence,
    human_connections: &dyn ConnectionPresence,
    backend_connections: &dyn ConnectionPresence,
    viewer_id: &ParticipantId,
    room_id_raw: &str,
    now: UtcMillis,
) -> Result<RoomLiveSnapshot, RoomQueryError> {
    let members = get_room_members(room, viewer_id, room_id_raw).await?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(RoomQueryError::NotFound)?;
    let typing = live.typing_in_room(&room_id, now);

    let members = members
        .into_iter()
        .map(|member| {
            let participant = member.participant;
            let online = match participant.kind {
                ParticipantKind::Human => human_connections.is_connected(&participant.id),
                ParticipantKind::Agent => {
                    backend_connections.is_connected(&participant.id)
                        || live.seen_recently(&participant.id, now)
                }
            };
            let typing = typing.iter().any(|id| id == &participant.id);
            MemberLive {
                participant_id: participant.id,
                display_name: participant.display_name,
                kind: participant.kind,
                online,
                typing,
            }
        })
        .collect();

    Ok(RoomLiveSnapshot { room_id, members })
}

/// 用例错误（对外统一形状）。
#[derive(Debug)]
pub enum LiveTypingError {
    /// 非该房 active 成员 / 未知 / bad-id（对外 `404 room_not_found`）。
    NotFound,
    /// 存储不可用（对外脱敏 500）。
    Store(RoomQueryError),
}

/// 声明「我正在这间房写」。
///
/// 裁决：viewer 必须是该房 active 成员（复用 `get_room_members`，与读面
/// **同一条**可见性判据——能看见这间房的人才谈得上在这间房写字）。写的是
/// 进程内易失槽，不产生任何公共事实、不产生事件、不进 Human Event 表。
pub async fn mark_typing(
    room: &dyn RoomStore,
    live: &dyn LivePresence,
    actor: &ParticipantId,
    room_id_raw: &str,
    now: UtcMillis,
) -> Result<(), LiveTypingError> {
    // 复用既有成员读取：它的 None 就是"你不是这房的人"，不另立判据。
    match get_room_members(room, actor, room_id_raw).await {
        Ok(_) => {}
        Err(RoomQueryError::NotFound) => return Err(LiveTypingError::NotFound),
        Err(e) => return Err(LiveTypingError::Store(e)),
    }
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(LiveTypingError::NotFound)?;
    live.mark_typing(&room_id, actor, now);
    Ok(())
}

/// 主体发出消息后清除自己的进行态（消息取代"正在写"）。
///
/// 这是**幂等的最佳努力**：没有进行态时无事发生；存储不可用也不影响消息
/// 这条主链（调用方只记录脱敏 warn，绝不因清理失败让已提交的消息报错）。
pub fn clear_own_typing(live: &dyn LivePresence, room_id: &RoomId, actor: &ParticipantId) {
    live.clear_typing(room_id, actor);
}

/// 「这个主体刚被看见过」：任何一次成功的 BackendAuth 都调它。
///
/// 它不写任何公共事实、不产生事件，只是让拉取式 Backend 也有一个"最近
/// 被看见"的兜底（挂门铃的 Backend 由连接本身给出在线）。
pub fn mark_seen(live: &dyn LivePresence, participant: &ParticipantId, now: UtcMillis) {
    live.mark_seen(participant, now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::live::{SEEN_TTL, TYPING_TTL};
    use crate::ports::live::LivePresence;
    use std::sync::Mutex;

    /// 极简假端口：只实现本模块用到的两条读路径。
    #[derive(Default)]
    struct FakeLive {
        seen: Mutex<Vec<(ParticipantId, UtcMillis)>>,
        typing: Mutex<Vec<(RoomId, ParticipantId, UtcMillis)>>,
    }

    impl LivePresence for FakeLive {
        fn mark_seen(&self, participant: &ParticipantId, now: UtcMillis) {
            self.seen.lock().unwrap().push((participant.clone(), now));
        }
        fn mark_typing(&self, room_id: &RoomId, participant: &ParticipantId, now: UtcMillis) {
            self.typing
                .lock()
                .unwrap()
                .push((room_id.clone(), participant.clone(), now));
        }
        fn clear_typing(&self, room_id: &RoomId, participant: &ParticipantId) {
            self.typing
                .lock()
                .unwrap()
                .retain(|(r, p, _)| !(r == room_id && p == participant));
        }
        fn typing_in_room(&self, room_id: &RoomId, now: UtcMillis) -> Vec<ParticipantId> {
            let window = TYPING_TTL.as_millis() as i64;
            self.typing
                .lock()
                .unwrap()
                .iter()
                .filter(|(r, _, at)| {
                    r == room_id && now.millis() < at.millis().saturating_add(window)
                })
                .map(|(_, p, _)| p.clone())
                .collect()
        }
        fn seen_recently(&self, participant: &ParticipantId, now: UtcMillis) -> bool {
            let window = SEEN_TTL.as_millis() as i64;
            self.seen.lock().unwrap().iter().any(|(p, at)| {
                p == participant && now.millis() < at.millis().saturating_add(window)
            })
        }
    }

    fn agent(raw: &str) -> ParticipantId {
        ParticipantId(raw.to_owned())
    }

    const ROOM: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn typing_is_scoped_by_room_and_actor() {
        let live = FakeLive::default();
        let room = RoomId::parse_canonical(ROOM).unwrap();
        let other_room = RoomId::parse_canonical("44444444-4444-4444-8444-444444444444").unwrap();
        let t0 = UtcMillis::from_millis(1_000);
        live.mark_typing(&room, &agent("a-1"), t0);
        live.mark_typing(&room, &agent("h-1"), t0);

        assert_eq!(
            live.typing_in_room(&room, t0).len(),
            2,
            "人与 Agent 同一条判据：都进同一个进行态表"
        );
        assert!(
            live.typing_in_room(&other_room, t0).is_empty(),
            "进行态按房隔离"
        );

        // 过期后（TTL 边界）自己消失，不需要任何人来清。
        let at_expiry = UtcMillis::from_millis(1_000 + TYPING_TTL.as_millis() as i64);
        assert!(live.typing_in_room(&room, at_expiry).is_empty());
    }

    #[test]
    fn clear_own_typing_is_idempotent() {
        let live = FakeLive::default();
        let room = RoomId::parse_canonical(ROOM).unwrap();
        let me = agent("a-1");
        live.mark_typing(&room, &me, UtcMillis::from_millis(1_000));
        clear_own_typing(&live, &room, &me);
        clear_own_typing(&live, &room, &me);
        assert!(
            live.typing_in_room(&room, UtcMillis::from_millis(1_000))
                .is_empty()
        );
    }

    #[test]
    fn mark_seen_makes_a_polling_backend_visible_but_only_within_the_window() {
        let live = FakeLive::default();
        let it = agent("a-1");
        let t0 = UtcMillis::from_millis(10_000);
        mark_seen(&live, &it, t0);
        assert!(live.seen_recently(&it, UtcMillis::from_millis(10_001)));
        assert!(!live.seen_recently(
            &it,
            UtcMillis::from_millis(10_000 + SEEN_TTL.as_millis() as i64)
        ));
    }
}
