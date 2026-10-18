//! 附件 HTTP handler（契约 CORE_V1_ATTACHMENTS.md 5.x/6）。
//!
//! 两条对称入口只做认证适配、共用同一 application 用例：
//! - Human 入口：`POST/GET /v1/rooms/{room_id}/attachments[/{attachment_id}]`
//!   + CurrentAuth → Human Participant；
//! - Backend 入口：`POST/GET /v1/backend/rooms/{room_id}/attachments/
//!   {attachment_id}` + BackendAuth → 认证 Agent。
//!
//! 上传是 multipart/form-data，严格形状：恰好一个 `name="file"` 文件 part
//! （Content-Disposition 必须带 filename，part Content-Type 必须存在）；
//! 文件字节 ≤ 16 MiB；路由级请求体上限 = 16 MiB + 64 KiB（multipart 框架
//! 开销），超出 413。下载授权后返回二进制 body + 固定安全响应头
//! （Content-Disposition: attachment、X-Content-Type-Options: nosniff、
//! Content-Type 为存储的 content_type）。所有非 2xx 走统一 JSON 错误信封。
//!
//! handler 只调用 `application::attachment` 用例与端口，不直接操作 SQLite
//! 或文件系统；公共响应绝不返回 storage key、SHA-256 或文件路径。

use axum::body::Body;
use axum::extract::multipart::{Multipart, MultipartError};
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::attachment::{
    DownloadAttachmentError, UploadAttachmentError, download_attachment, upload_attachment,
};
use crate::domain::attachment::MAX_ATTACHMENT_BYTES;
use crate::protocol::attachment::{UploadAttachmentResponse, UploadAttachmentView};
use crate::protocol::error::codes;

/// 上传路由级请求体上限：16 MiB 文件字节 + 64 KiB multipart 框架开销。
pub const MAX_ATTACHMENT_REQUEST_BYTES: usize = MAX_ATTACHMENT_BYTES + 64 * 1024;

/// 附件切片路由：Human 与 Backend 两套对称入口；上传路由带独立的大 body
/// limit（16 MiB + 开销），不沿用 Message 路由的 128 KiB。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/rooms/{room_id}/attachments",
            post(upload_attachment_route),
        )
        .route(
            "/v1/rooms/{room_id}/attachments/{attachment_id}",
            get(download_attachment_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_ATTACHMENT_REQUEST_BYTES,
        ))
        .route(
            "/v1/backend/rooms/{room_id}/attachments",
            post(upload_backend_attachment_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/attachments/{attachment_id}",
            get(download_backend_attachment_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_ATTACHMENT_REQUEST_BYTES,
        ))
}

/// multipart 解析错误 → 413（字节超限）或 400（形状错误）。
fn multipart_error_response(e: MultipartError, request_id: &str) -> Response {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            codes::PAYLOAD_TOO_LARGE,
            format!("request body exceeds {MAX_ATTACHMENT_REQUEST_BYTES} bytes"),
            request_id,
        )
    } else {
        error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid multipart upload request",
            request_id,
        )
    }
}

/// 上传公共逻辑（Human 与 Backend 两个入口共用）：严格 multipart 形状 →
/// 元数据与字节边界校验 → application 用例。恰好一个 `name="file"` 文件
/// part；额外 part / 缺文件 part / 缺 filename / 缺 Content-Type / 文件
/// 字节超 16 MiB 一律拒绝。
async fn upload_common(
    state: &AppState,
    uploader_id: &crate::domain::participant::ParticipantId,
    room_id: &str,
    request_id: &str,
    mut multipart: Multipart,
) -> Response {
    let mut file: Option<(String, String, axum::body::Bytes)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() != Some("file") || file.is_some() {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        codes::INVALID_REQUEST,
                        "upload request must contain exactly one file part",
                        request_id,
                    );
                }
                let Some(file_name) = field.file_name().map(str::to_owned) else {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        codes::INVALID_ATTACHMENT_METADATA,
                        "file part must carry a file name",
                        request_id,
                    );
                };
                let Some(content_type) = field.content_type().map(str::to_owned) else {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        codes::INVALID_ATTACHMENT_METADATA,
                        "file part must carry a content type",
                        request_id,
                    );
                };
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(e) => return multipart_error_response(e, request_id),
                };
                if bytes.len() > MAX_ATTACHMENT_BYTES {
                    return error_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        codes::PAYLOAD_TOO_LARGE,
                        format!("attachment exceeds {MAX_ATTACHMENT_BYTES} bytes"),
                        request_id,
                    );
                }
                file = Some((file_name, content_type, bytes));
            }
            Ok(None) => break,
            Err(e) => return multipart_error_response(e, request_id),
        }
    }
    let Some((file_name, content_type, bytes)) = file else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "upload request must contain a file part",
            request_id,
        );
    };

    match upload_attachment(
        state.attachments.as_ref(),
        state.blob.as_ref(),
        uploader_id,
        room_id,
        &file_name,
        &content_type,
        &bytes,
    )
    .await
    {
        Ok(outcome) => {
            let attachment = &outcome.attachment;
            (
                StatusCode::CREATED,
                Json(UploadAttachmentResponse {
                    attachment: UploadAttachmentView {
                        id: attachment.id.as_str().to_owned(),
                        file_name: attachment.file_name.as_str().to_owned(),
                        content_type: attachment.content_type.as_str().to_owned(),
                        size_bytes: attachment.size_bytes,
                        state: attachment.state.as_str(),
                        uploaded_at: attachment.uploaded_at.to_rfc3339(),
                    },
                    request_id: request_id.to_owned(),
                }),
            )
                .into_response()
        }
        Err(e) => upload_error_response(e, request_id),
    }
}

/// 上传错误映射：bad/unknown/非成员 Room 统一 404 room_not_found；元数据
/// 非法 400 invalid_attachment_metadata；超限 413；存储按既有安全边界。
fn upload_error_response(e: UploadAttachmentError, request_id: &str) -> Response {
    match e {
        UploadAttachmentError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        UploadAttachmentError::InvalidAttachmentMetadata(message) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_ATTACHMENT_METADATA,
            message,
            request_id,
        ),
        UploadAttachmentError::PayloadTooLarge => error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            codes::PAYLOAD_TOO_LARGE,
            format!("attachment exceeds {MAX_ATTACHMENT_BYTES} bytes"),
            request_id,
        ),
        UploadAttachmentError::Store(e) => {
            store_error_response("attachment.upload", &e, request_id)
        }
    }
}

/// POST /v1/rooms/{room_id}/attachments（Human Session）。
async fn upload_attachment_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    multipart: Multipart,
) -> Response {
    upload_common(
        &state,
        &auth.participant.id,
        &room_id,
        &request_id.0,
        multipart,
    )
    .await
}

/// POST /v1/backend/rooms/{room_id}/attachments（Backend Credential）：
/// uploader 为认证 Agent。
async fn upload_backend_attachment_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    multipart: Multipart,
) -> Response {
    upload_common(
        &state,
        &auth.view.agent.id,
        &room_id,
        &request_id.0,
        multipart,
    )
    .await
}

/// 下载公共逻辑（Human 与 Backend 两个入口共用）：授权裁决（None → 统一
/// 404 attachment_not_found）→ BlobStorage 读取 → 固定安全响应头。
async fn download_common(
    state: &AppState,
    reader_id: &crate::domain::participant::ParticipantId,
    room_id: &str,
    attachment_id: &str,
    request_id: &str,
) -> Response {
    match download_attachment(
        state.attachments.as_ref(),
        state.blob.as_ref(),
        reader_id,
        room_id,
        attachment_id,
    )
    .await
    {
        Ok((metadata, bytes)) => {
            let mut response = Response::new(Body::from(bytes));
            // content_type 已通过上传校验（ASCII 可见 1-255 bytes），可安全
            // 放入响应头；解析失败属内部不变量破坏。
            let content_type = HeaderValue::from_str(metadata.content_type.as_str())
                .expect("stored content type must be a valid header value");
            response.headers_mut().insert(CONTENT_TYPE, content_type);
            // Content-Disposition 固定为安全 attachment：不携带任何用户输入
            // filename，file_name 只经公共元数据 API 返回。
            response
                .headers_mut()
                .insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
            response
                .headers_mut()
                .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            response
        }
        Err(e) => download_error_response(e, request_id),
    }
}

/// 下载错误映射：bad room / bad attachment UUID 400 invalid_request；
/// pending/unknown/跨 Room/跨 actor/过期/撤回/非成员统一 404
/// attachment_not_found（不泄露存在性或原因）；存储按既有安全边界。
fn download_error_response(e: DownloadAttachmentError, request_id: &str) -> Response {
    match e {
        DownloadAttachmentError::InvalidRequest => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "room_id and attachment_id must be canonical UUID v4",
            request_id,
        ),
        DownloadAttachmentError::AttachmentNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ATTACHMENT_NOT_FOUND,
            "attachment not found",
            request_id,
        ),
        DownloadAttachmentError::Store(e) => {
            store_error_response("attachment.download", &e, request_id)
        }
    }
}

/// GET /v1/rooms/{room_id}/attachments/{attachment_id}（Human Session）。
async fn download_attachment_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((room_id, attachment_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    download_common(
        &state,
        &auth.participant.id,
        &room_id,
        &attachment_id,
        &request_id.0,
    )
    .await
}

/// GET /v1/backend/rooms/{room_id}/attachments/{attachment_id}（Backend
/// Credential）：读取者/下载者为认证 Agent（与 Human 同一成员/窗口授权
/// 谓词）。
async fn download_backend_attachment_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path((room_id, attachment_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    download_common(
        &state,
        &auth.view.agent.id,
        &room_id,
        &attachment_id,
        &request_id.0,
    )
    .await
}
