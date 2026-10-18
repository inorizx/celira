//! 公开请求、响应、事件和错误模型。
//!
//! protocol 不依赖具体数据库、HTTP 框架或任何外部 Agent 实现。

pub mod agent;
pub mod attachment;
pub mod backend;
pub mod call;
pub mod contact;
pub mod cursor;
pub mod directory;
pub mod error;
pub mod event;
pub mod home;
pub mod human_event;
pub mod human_profile;
pub mod live;
pub mod me;
pub mod message;
pub mod participant_avatar;
pub mod participant_presentation;
pub mod room;
