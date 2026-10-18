//! 领域实体、状态、不变量和纯规则。
//!
//! 本模块是 Core 领域模型的唯一归属地。它不得依赖 HTTP、WebSocket、SQLite、
//! 任何外部 Agent 实现或外部框架。

pub mod agent;
pub mod agent_event;
pub mod attachment;
pub mod backend;
pub mod call;
pub mod contact;
pub mod grant_token;
pub mod home;
pub mod human_event;
pub mod human_session;
pub mod live;
pub mod message;
pub mod pairing_transaction;
pub mod participant;
pub mod participant_avatar;
pub mod participant_presentation;
pub mod room;
pub mod time;
pub mod token;

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod participant_presentation_tests;
#[cfg(test)]
mod participant_tests;
#[cfg(test)]
mod time_tests;
#[cfg(test)]
mod token_tests;
