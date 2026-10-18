//! adapter：具体技术实现（HTTP、SQLite、本地 Blob 存储）。

pub mod blob;
pub mod call;
pub mod http;
pub mod live;
pub mod notifier;
pub mod sqlite;
