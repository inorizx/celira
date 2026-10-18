//! 附件公共协议模型（契约 CORE_V1_ATTACHMENTS.md 5.x）。
//!
//! 公共视图只包含 id/file_name/content_type/size_bytes（上传响应另含
//! state 与 uploaded_at）；storage key、SHA-256 与文件系统路径永不进入
//! 任何公共响应。

use serde::Serialize;

/// Message 视图内的附件元数据元素（MessageView.attachments 数组项）。
#[derive(Debug, Clone, Serialize)]
pub struct AttachmentMetadataView {
    pub id: String,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
}

/// POST /v1/rooms/{room_id}/attachments 成功响应：pending 附件公共事实。
/// 不返回 storage key、hash 或路径。
#[derive(Debug, Clone, Serialize)]
pub struct UploadAttachmentResponse {
    pub attachment: UploadAttachmentView,
    pub request_id: String,
}

/// 上传响应的附件视图：state 恒为 "pending"。
#[derive(Debug, Clone, Serialize)]
pub struct UploadAttachmentView {
    pub id: String,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub state: &'static str,
    pub uploaded_at: String,
}
