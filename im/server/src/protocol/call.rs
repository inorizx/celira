//! Call 协议模型（契约 CORE_V1_CALL.md）。
//!
//! 本协议面只处理临时实时协调事实：发起/接受/拒接/挂断/信令中继的状态机
//! 与两个 call-stream（Human Session + Backend Credential）。媒体（音频/
//! 视频）是两端客户端 WebRTC P2P 直连，Core 从不承载/存储媒体字节，本协议
//! 面只中继纯信令控制消息并协调 Call 状态；`media` 只是振铃/UI 展示的提示
//! 字段，Core 不解析媒体、不从 SDP 推断。

use serde::{Deserialize, Serialize};

use crate::domain::call::{Call, CallMedia, IceCandidate, SignalKind};

/// POST /v1/calls 与 POST /v1/backend/calls 请求：在共享 Room 内发起点到点
/// 通话。`media` 可选（缺省 `audio`）：仅振铃/UI 展示用，Core 不解析媒体。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitiateRequest {
    pub room_id: String,
    pub callee_id: String,
    /// audio | video；缺省 audio。
    #[serde(default)]
    pub media: Option<String>,
}

/// POST /v1/calls/{call_id}/signal 请求：中继信令至对方。
/// - offer/answer：`sdp`（单个 SDP 字符串，1..=16384 UTF-8 bytes，禁 NUL）；
/// - ice：`candidates`（1..=32 条，每条 1..=2048 bytes，禁 NUL）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalRequest {
    /// offer | answer | ice
    pub kind: String,
    #[serde(default)]
    pub sdp: Option<String>,
    #[serde(default)]
    pub candidates: Option<Vec<IceCandidate>>,
}

/// Call 的公共投影：只含最小公共身份与状态，绝不暴露 Credential/Binding/
/// token/内部 sequence。
#[derive(Debug, Clone, Serialize)]
pub struct CallView {
    pub call_id: String,
    pub room_id: String,
    pub caller_id: String,
    pub caller_display: String,
    pub callee_id: String,
    pub callee_display: String,
    /// audio | video（振铃/UI 提示；不影响媒体面）。
    pub media: String,
    /// ringing | in_progress | ended
    pub state: String,
    /// hangup_caller | hangup_callee | decline_callee | timeout_ring | null
    pub ended_reason: Option<String>,
    pub created_at: String,
    pub accepted_at: Option<String>,
    pub ended_at: Option<String>,
}

impl CallView {
    pub fn from(call: &Call) -> Self {
        Self {
            call_id: call.id.as_str().to_owned(),
            room_id: call.room_id.as_str().to_owned(),
            caller_id: call.caller_id.as_str().to_owned(),
            caller_display: call.caller_display.as_str().to_owned(),
            callee_id: call.callee_id.as_str().to_owned(),
            callee_display: call.callee_display.as_str().to_owned(),
            media: call.media.as_str().to_owned(),
            state: call.state.as_str().to_owned(),
            ended_reason: call.ended_reason.map(|r| r.as_str()).map(str::to_owned),
            created_at: call.created_at.to_rfc3339(),
            accepted_at: call.accepted_at.map(|t| t.to_rfc3339()),
            ended_at: call.ended_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// POST /v1/calls 响应：Call + 是否新建（同 (room, caller, callee) 既有
/// active Call 复用 → created=false）。
#[derive(Debug, Clone, Serialize)]
pub struct InitiateResponse {
    pub call: CallView,
    pub created: bool,
    pub request_id: String,
}

/// GET /v1/calls/{call_id} 响应。
#[derive(Debug, Clone, Serialize)]
pub struct CallResponse {
    pub call: CallView,
    pub request_id: String,
}

/// 动作（accept/decline/hangup）成功响应。
#[derive(Debug, Clone, Serialize)]
pub struct CallActionResponse {
    pub call: CallView,
    pub request_id: String,
}

/// 信号请求中的 kind 字符串（协议判别值）。
pub mod signal_kind {
    pub const OFFER: &str = "offer";
    pub const ANSWER: &str = "answer";
    pub const ICE: &str = "ice";
}

/// 发起请求中的 media 字符串（协议判别值；缺省 audio）。
pub mod media_kind {
    pub const AUDIO: &str = "audio";
    pub const VIDEO: &str = "video";
}

/// 由发起请求解析媒体提示：`None` → 缺省 `audio`；显式值非法 → None（发起
/// 方路由 400 invalid_request，不泄露细节）。
pub fn resolve_media(value: Option<&str>) -> Option<CallMedia> {
    match value {
        None | Some("") => Some(CallMedia::Audio),
        Some(media_kind::AUDIO) => Some(CallMedia::Audio),
        Some(media_kind::VIDEO) => Some(CallMedia::Video),
        Some(_) => None,
    }
}

/// 由 SignalRequest 构造领域 `SignalKind`。
pub fn parse_signal_kind(value: &str) -> Option<SignalKind> {
    match value {
        signal_kind::OFFER => Some(SignalKind::Offer),
        signal_kind::ANSWER => Some(SignalKind::Answer),
        signal_kind::ICE => Some(SignalKind::Ice),
        _ => None,
    }
}

/// 由 SignalRequest 构造已校验的领域信令载荷；形状/字节/NUL/数量不合法
/// 返回 None（→ 400 invalid_request / call_payload_invalid）。
pub fn build_signal_body(
    kind: SignalKind,
    req: &SignalRequest,
) -> Option<crate::domain::call::SignalBody> {
    if kind == SignalKind::Ice {
        return crate::domain::call::SignalBody::parse_ice(req.candidates.clone()?);
    }
    let payload = req.sdp.clone().map(|s| vec![s])?;
    crate::domain::call::SignalBody::parse(kind, payload)
}

/// Call-stream WS 事件帧的固定 `type` 值。
pub const WS_CALL_RING: &str = "call.ring";
pub const WS_CALL_SIGNAL: &str = "call.signal";
pub const WS_CALL_STATE: &str = "call.state";

/// CallStreamEvent → 序列化为 WS 帧文本（只含最小公共投影）。
pub fn call_stream_frame(event: &crate::ports::call::CallStreamEvent) -> String {
    match event {
        crate::ports::call::CallStreamEvent::Ring { call } => {
            serde_json::json!({ "type": WS_CALL_RING, "call": CallView::from(call) }).to_string()
        }
        crate::ports::call::CallStreamEvent::Signal {
            call_id,
            from,
            body,
            ..
        } => {
            let mut payload = serde_json::Map::new();
            match body.kind {
                SignalKind::Offer | SignalKind::Answer => {
                    payload.insert(
                        "sdp".into(),
                        serde_json::Value::String(body.single().to_owned()),
                    );
                }
                SignalKind::Ice => {
                    payload.insert(
                        "candidates".into(),
                        serde_json::Value::Array(
                            body.ice_candidates
                                .as_ref()
                                .map(|candidates| {
                                    candidates
                                        .iter()
                                        .map(|candidate| {
                                            serde_json::json!({
                                                "candidate": candidate.candidate,
                                                "sdp_mid": candidate.sdp_mid,
                                                "sdp_mline_index": candidate.sdp_mline_index,
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_else(|| {
                                    body.payload
                                        .iter()
                                        .map(|candidate| serde_json::json!({ "candidate": candidate }))
                                        .collect()
                                }),
                        ),
                    );
                }
            }
            serde_json::json!({
                "type": WS_CALL_SIGNAL,
                "call_id": call_id.as_str(),
                "from_id": from.as_str(),
                "kind": body.kind.as_str(),
                "payload": payload,
            })
            .to_string()
        }
        crate::ports::call::CallStreamEvent::StateChanged { call } => {
            serde_json::json!({ "type": WS_CALL_STATE, "call": CallView::from(call) }).to_string()
        }
    }
}
