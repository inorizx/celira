//! Agent 通讯录（Directory）读取 API 的请求与响应模型
//! （契约 CORE_V1_AGENT_DIRECTORY.md，切片 B1「看见自己的世界」）。
//!
//! `GET /v1/backend/rooms` 是**纯读**端点：BackendAuth 认证，返回该 agent
//! 为 active member 的房间目录（每房最近一条公共预览 + since 增量计数）。
//! 模型只含公共投影，不携带 token、幂等键、Membership 内部 sequence 或
//! Runtime 私有状态；预览与计数都从既有公共 Message 事实推导，不新增事实。

use serde::{Deserialize, Serialize};

use crate::protocol::room::ReadStateView;

/// `GET /v1/backend/rooms` 的查询参数。`since` 可选：unix ms 时间戳，
/// 缺省 = 无增量基线（`since_count` 恒为 0）；非负整数否则 400
/// `invalid_request`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DirectoryQuery {
    /// unix ms 时间戳（`i64` 十进制；负值/溢出/非数字 → 400）。
    pub since: Option<i64>,
}

/// 单房目录行（全机械投影，见契约 §4）：
///
/// - `room_id`：规范小写连字符 UUID v4；
/// - `kind`：`direct` / `human_direct` / `group`；
/// - `display_name`：机械显示名 = 房间标题（`name`，group 恒有、direct
///   可选）否则对端 Participant display_name（direct/human_direct）——
///   `COALESCE(NULLIF(r.name,''), peer.display_name)`；
/// - `last_activity_at`：该房**对该 agent 可见**的最近一条公共 Message 的
///   created_at（RFC3339）；无可见消息 → `null`（排序时回退房间 created_at）；
/// - `preview`：该最近一条 Message 的公共正文投影（撤回 tombstone /
///   附件-only → `null`；Core 不截断——Runtime 呈现面自行机械截断）；
/// - `since_count`：自 `since` 以来该房对该 agent 可见、且 author ≠ 该
///   agent 的公共 Message 增量计数（同一成员可见性口径，见契约 §4）；
/// - `read_state`：该 agent 在这个房间的**读位置与未读**（切片 M1，契约
///   `CORE_V1_AGENT_READ_CURSOR.md` §3）——与 Human 侧房间列表的
///   `read_state` **同一投影**：`{last_read_message_id, unread_count}`。
///   ⚠ **位置本身仍然只属于该 agent 自己**（推进不产生任何事件）；
///   2026-10-10 起多出下面那一格，它是**从位置派生的一个布尔**，不是位置本身；
/// - `peer_read`：「**对方读没读我最后那条**」（契约 `CORE_V1_READ_RECEIPT.md`
///   §2，与 Human 侧**同一格、同一判据**）：`"read"` / `"unread"`；
///   **不出现**（`null`）＝ 问题不成立——房不是 `direct`、对端当前不是这间房的
///   active 成员、或该 agent 在这间房还没说过话。⚠ 它是**读位置**，不是
///   "对方看到了"；
/// - `attention`：**提醒档**（契约 `CORE_V1_IM_CONTRACT.md` §9.1，本行由
///   2026-10-19 增补）：`"all"` / `"mentions"`——这间房的动静要不要提醒我。
///   **缺行投影成 `all`**（不预建行）；两人房里没有这一档，恒为 `all`。
///   与 Human 侧房间列表的 `attention` **同一投影**；`since` / `since_count` /
///   `read_state` 口径全部不受影响。
#[derive(Debug, Clone, Serialize)]
pub struct BackendRoomView {
    pub room_id: String,
    pub kind: String,
    pub display_name: String,
    pub last_activity_at: Option<String>,
    pub preview: Option<String>,
    pub since_count: u64,
    pub read_state: ReadStateView,
    pub peer_read: Option<String>,
    pub attention: String,
}

/// `GET /v1/backend/rooms` 成功响应：房间按机械最近活动时间倒序
/// （无消息房按房间创建时间），再按房间 ID 升序。
#[derive(Debug, Clone, Serialize)]
pub struct BackendRoomsResponse {
    pub rooms: Vec<BackendRoomView>,
    pub request_id: String,
}
