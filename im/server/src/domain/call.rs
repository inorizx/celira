//! Call：点到点 WebRTC 实时协调的领域切片（进程内有界、可过期、非持久
//! 投影）。
//!
//! 权威契约：CORE_V1_CALL.md（Core v1 窄解冻切片）。
//!
//! 本切片只表达「通话实时协调」的临时事实，与 Agent Live State 同质：
//! - Call 状态机（ringing → in_progress → ended）只存在于 Core 进程内，
//!   **绝不写 SQLite**，不产生/改写 Message、Human Event、Agent Event、
//!   Completion、ACK、Membership、Permission、Binding、Credential；
//! - 媒体（音频/视频）由两端客户端 WebRTC P2P 直连，**Core 永不接触媒体
//!   字节**，只协调状态并中继纯信令控制消息（SDP/ICE）。`media` 只是振铃/
//!   UI 展示的提示字段，Core 不解析媒体、不从 SDP 推断；
//! - 发起方与应答方都既可以是 Human 也可以是 Agent（任意两个 active
//!   Participant 都可互相呼叫；Human 经 Human Session、Agent 经 Backend
//!   Credential）；
//! - 一切非持久、有界、可过期；ring 超时由 Core 固定；Core 重启即失忆
//!   （fail closed）。

use std::fmt;

use crate::domain::participant::{DisplayName, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// ring 超时（Core 固定，不接受调用方自定义）：呼入后 callee 未应答即
/// `ended(timeout_ring)`。媒体面 P2P 不受此限。
pub const RING_TTL: std::time::Duration = std::time::Duration::from_secs(45);

/// 后台主动 sweep 的固定短间隔（Core 固定；测试可注入更短间隔的 helper）：
/// 明显小于 RING_TTL，保证 ring 超时最迟约一个间隔内被单一清理器报告。
pub const CALL_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// 本进程内同时活跃的 Call 数硬上限（fail closed `call_state_limit_exceeded`）。
pub const MAX_CALLS: usize = 4096;

/// 单条 SDP（offer/answer）的 UTF-8 字节上限（不 trim、不解析）。
pub const MAX_SDP_BYTES: usize = 16384;
/// 单条 ICE candidate 的 UTF-8 字节上限。
pub const MAX_ICE_CANDIDATE_BYTES: usize = 2048;
/// ICE candidate 的媒体段标识上限。
pub const MAX_ICE_SDP_MID_BYTES: usize = 128;
/// ICE candidate 的媒体段索引上限。
pub const MAX_ICE_SDP_MLINE_INDEX: u32 = 64;
/// 单条信令里携带的 ICE candidate 数上限。
pub const MAX_ICE_CANDIDATES: usize = 32;

/// 通话 ID：稳定、不可复用的 UUID v4 字符串（统一小写连字符）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallId(pub String);

impl CallId {
    /// 解析并规范化为小写连字符的 UUID v4；其他格式返回 None（对外统一
    /// 404/400 invalid_request）。
    pub fn parse_canonical(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random))
            .then(|| Self(uuid.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 服务端签约新 Call 时生成。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Call 状态机：`ringing → in_progress → ended`；`ended` 是终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    /// 已呼出、等待 callee 应答。
    Ringing,
    /// callee 已接受，媒体由两端 P2P 直连。
    InProgress,
    /// 已结束（原因见 `ended_reason`）。
    Ended,
}

impl CallState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ringing => "ringing",
            Self::InProgress => "in_progress",
            Self::Ended => "ended",
        }
    }
}

/// 结束原因（`state == ended` 时非空）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedReason {
    HangupCaller,
    HangupCallee,
    DeclineCallee,
    TimeoutRing,
}

impl EndedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HangupCaller => "hangup_caller",
            Self::HangupCallee => "hangup_callee",
            Self::DeclineCallee => "decline_callee",
            Self::TimeoutRing => "timeout_ring",
        }
    }
}

/// 信令消息种类：SDP offer / SDP answer / ICE candidate（trickle）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Offer,
    Answer,
    Ice,
}

impl SignalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Answer => "answer",
            Self::Ice => "ice",
        }
    }
}

/// WebRTC ICE candidate 的最小路由元数据。Core 只做有界中继，不解析
/// candidate 文本；接收端用媒体段元数据恢复浏览器原生 CandidateInit。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IceCandidate {
    pub candidate: String,
    #[serde(default)]
    pub sdp_mid: Option<String>,
    #[serde(default)]
    pub sdp_mline_index: Option<u32>,
}

/// 一条已通过字节/NUL/数量边界校验的信令载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalBody {
    pub kind: SignalKind,
    /// offer/answer 为单个 SDP 字符串；ice 为候选列表（升序、去重由调用方
    /// 保证）；均不 trim、不解析。
    pub payload: Vec<String>,
    /// 结构化 ICE 的媒体段元数据；SDP 信令为 None。
    pub ice_candidates: Option<Vec<IceCandidate>>,
}

impl SignalBody {
    /// 校验并构造（不解析内容）：UTF-8 字节边界、禁 NUL、数量上限。失败
    /// 返回 None → 对外 400 invalid_request（不泄露细节）。
    pub fn parse(kind: SignalKind, payload: Vec<String>) -> Option<Self> {
        match kind {
            SignalKind::Offer | SignalKind::Answer => {
                if payload.len() != 1 {
                    return None;
                }
                validate_sdp(&payload[0]).then_some(Self {
                    kind,
                    payload,
                    ice_candidates: None,
                })
            }
            SignalKind::Ice => {
                if payload.is_empty() || payload.len() > MAX_ICE_CANDIDATES {
                    return None;
                }
                payload
                    .iter()
                    .all(|c| valid_bytes(c, MAX_ICE_CANDIDATE_BYTES))
                    .then_some(Self {
                        kind,
                        payload,
                        ice_candidates: None,
                    })
            }
        }
    }

    /// 构造 HTTP/JSON 使用的结构化 ICE 载荷。
    pub fn parse_ice(candidates: Vec<IceCandidate>) -> Option<Self> {
        if candidates.is_empty() || candidates.len() > MAX_ICE_CANDIDATES {
            return None;
        }
        if !candidates.iter().all(|candidate| {
            valid_bytes(&candidate.candidate, MAX_ICE_CANDIDATE_BYTES)
                && candidate
                    .sdp_mid
                    .as_deref()
                    .is_none_or(|mid| valid_bytes(mid, MAX_ICE_SDP_MID_BYTES))
                && candidate
                    .sdp_mline_index
                    .is_none_or(|index| index <= MAX_ICE_SDP_MLINE_INDEX)
        }) {
            return None;
        }
        Some(Self {
            kind: SignalKind::Ice,
            payload: candidates
                .iter()
                .map(|candidate| candidate.candidate.clone())
                .collect(),
            ice_candidates: Some(candidates),
        })
    }

    /// 第一个载荷（offer/answer 的 SDP）。
    pub fn single(&self) -> &str {
        &self.payload[0]
    }
}

fn validate_sdp(sdp: &str) -> bool {
    valid_bytes(sdp, MAX_SDP_BYTES)
}

fn valid_bytes(s: &str, max: usize) -> bool {
    let bytes = s.len();
    (1..=max).contains(&bytes) && !s.as_bytes().contains(&0)
}

/// 通话媒体提示（振铃/UI 展示用；Core 不解析媒体、不从 SDP 推断）。
/// 媒体字节始终由两端客户端 WebRTC P2P 直连，Core 永不接触。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallMedia {
    Audio,
    Video,
}

impl CallMedia {
    /// 目前仅被单元测试消费（生产路径经 serde 反序列化进入）；限测试构建以避免 dead_code 警告。
    #[cfg(test)]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "audio" => Some(Self::Audio),
            "video" => Some(Self::Video),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

/// 一条 Call 的进程内投影（非持久；Core 重启即失忆）。
#[derive(Debug, Clone)]
pub struct Call {
    pub id: CallId,
    pub room_id: RoomId,
    pub caller_id: ParticipantId,
    pub caller_display: DisplayName,
    pub callee_id: ParticipantId,
    pub callee_display: DisplayName,
    /// audio | video（振铃/UI 提示；不影响媒体面）。
    pub media: CallMedia,
    pub state: CallState,
    pub ended_reason: Option<EndedReason>,
    pub created_at: UtcMillis,
    pub accepted_at: Option<UtcMillis>,
    pub ended_at: Option<UtcMillis>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_id_parses_canonical_uuid_v4_only() {
        assert!(CallId::parse_canonical("00000000-0000-4000-8000-000000000001").is_some());
        for bad in [
            "",
            "not-a-uuid",
            "00000000-0000-1000-8000-000000000001", // v1
            "00000000-0000-5000-8000-000000000001", // v5
        ] {
            assert!(CallId::parse_canonical(bad).is_none(), "reject {bad:?}");
        }
    }

    #[test]
    fn state_and_reason_strings() {
        assert_eq!(CallState::Ringing.as_str(), "ringing");
        assert_eq!(CallState::InProgress.as_str(), "in_progress");
        assert_eq!(CallState::Ended.as_str(), "ended");
        assert_eq!(EndedReason::HangupCaller.as_str(), "hangup_caller");
        assert_eq!(EndedReason::TimeoutRing.as_str(), "timeout_ring");
    }

    #[test]
    fn signal_body_bounds_and_nul() {
        assert!(SignalBody::parse(SignalKind::Offer, vec!["v=0".into()]).is_some());
        // SDP 恰好上限合法；超限拒绝（不截断）。
        let max = "a".repeat(MAX_SDP_BYTES);
        assert!(SignalBody::parse(SignalKind::Answer, vec![max]).is_some());
        let over = "a".repeat(MAX_SDP_BYTES + 1);
        assert!(SignalBody::parse(SignalKind::Answer, vec![over]).is_none());
        // offer/answer 必须恰好一条。
        assert!(SignalBody::parse(SignalKind::Offer, vec![]).is_none());
        assert!(SignalBody::parse(SignalKind::Offer, vec!["a".into(), "b".into()]).is_none());
        // NUL 拒绝。
        assert!(SignalBody::parse(SignalKind::Offer, vec!["a\u{0}b".into()]).is_none());
        // ICE：非空、数量上限、每条字节上限。
        assert!(SignalBody::parse(SignalKind::Ice, vec!["c1".into()]).is_some());
        assert!(SignalBody::parse(SignalKind::Ice, vec![]).is_none());
        let many: Vec<String> = (0..=MAX_ICE_CANDIDATES).map(|i| format!("c{i}")).collect();
        assert!(SignalBody::parse(SignalKind::Ice, many).is_none());
        let long = "a".repeat(MAX_ICE_CANDIDATE_BYTES);
        assert!(SignalBody::parse(SignalKind::Ice, vec![long]).is_some());
    }

    #[test]
    fn media_parse_round_trip() {
        assert_eq!(CallMedia::parse("audio"), Some(CallMedia::Audio));
        assert_eq!(CallMedia::parse("video"), Some(CallMedia::Video));
        assert_eq!(CallMedia::Audio.as_str(), "audio");
        assert_eq!(CallMedia::Video.as_str(), "video");
        for bad in ["", "voice", "screen", "AUDIO"] {
            assert!(CallMedia::parse(bad).is_none(), "reject {bad:?}");
        }
    }
}
