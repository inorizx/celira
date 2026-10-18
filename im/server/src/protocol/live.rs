//! 主体进行态协议模型（契约 `CORE_V1_AGENT_PROTOCOL.md` §6）。
//!
//! 协议面上只有两件事：
//! - **读**：一间房的进行态快照——该房**全部 active 成员**（人与 Agent
//!   同形），每行 `{participant_id, display_name, kind, online, typing}`；
//! - **写**：一条无参数的 typing 声明「我正在这间房写」。
//!
//! 这里**没有**执行面字段：没有阶段（thinking/working/responding）、没有
//! 生成流 id、没有 revision、没有草稿正文、没有 instance id。那些随旧
//! Live State 于 2026-09-28 删除，且不打算回来。
//!
//! 两条纪律照旧：进行态绝不进入 `/v1/human/events`、Human Event 表或
//! `events.available`；读面可见性 = 该房 active 成员（沿用既有成员读取
//! 用例，unknown / bad-id / 非成员统一 `404 room_not_found`）。

use serde::Serialize;

/// GET /v1/rooms/{room_id}/live 与 GET /v1/backend/rooms/{room_id}/live
/// 的成功响应（两条端点**同一投影**，只在适配器换认证面）。
#[derive(Debug, Clone, Serialize)]
pub struct RoomLiveSnapshotResponse {
    pub room_id: String,
    pub members: Vec<MemberLiveView>,
    pub request_id: String,
}

/// 成员一行的进行态投影。`kind` 只用于呈现（人与 Agent 同形）；
/// `online` / `typing` 都是**易失投影**，不是耐久事实。
#[derive(Debug, Clone, Serialize)]
pub struct MemberLiveView {
    pub participant_id: String,
    pub display_name: String,
    pub kind: String,
    pub online: bool,
    pub typing: bool,
}
