//! 面向存储、时钟、ID、事件交付的窄接口。
//!
//! ports 只声明应用层需要的边界，不依赖具体数据库、HTTP 框架或任何外部 Agent 实现。

pub mod agent;
pub mod attachment;
pub mod backend;
pub mod blob;
pub mod call;
pub mod contact;
pub mod event;
pub mod home;
pub mod human_event;
pub mod identity;
pub mod live;
pub mod message;
pub mod migration;
pub mod participant_presentation;
pub mod room;
pub mod storage;
