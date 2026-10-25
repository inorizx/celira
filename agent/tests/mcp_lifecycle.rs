//! MCP 通道租约（2026-10-25）：真进程验收。
//!
//! 三条判据，全部用**真实 pid** 说话：
//!
//! - 两段 Run（两个 `McpApp`，同一份 access/池）复用**同一条通道**（pid 相同）；
//! - `,idle=1` 的一台闲够之后被宿主 `sweep` 收掉，下一段展开拿到**新 pid**；
//! - `,keep-alive` 的一台过多久都不换 pid；删掉声明后 `retain` 会把它收掉。
//!
//! 这里不碰 Core / Run / Turn，也不验证模型上下文——租约只管"端点活多久"。

use std::time::Duration;

use celira_agent::app_contract::AppPort;
use celira_agent::core::{Action, ActionId, ActionParams, ActionPort};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::mcp::{ChannelPool, McpAccess, McpApp, parse_declaration};

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_mcp_fixture_server")
}

fn agent() -> AgentId {
    AgentId::new("agent-a")
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("run-1"), Turn::FIRST)
}

fn receipt(outcome: &celira_agent::core::ActionOutcome) -> String {
    outcome
        .content()
        .first()
        .map(|block| block.text().to_string())
        .unwrap_or_default()
}

/// 展开一次、调一次 `where`、把夹具报的 pid 取出来。`McpApp` 在返回时被丢掉——下一次调用
/// 就是一个新的"Run 面"，但它和上一次共享同一份 access / 池。
fn pid_of_one_run(access: McpAccess) -> u32 {
    let mut app = McpApp::new(access);
    app.present().expect("夹具可以展开");
    let outcome = app.execute(
        &agent(),
        &operation(),
        &Action::with_params(ActionId::new("lab_where"), ActionParams::none()),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    let text = receipt(&outcome);
    let pid = text
        .split("pid=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("回执里没有 pid：{text}"));
    pid.parse()
        .unwrap_or_else(|_| panic!("pid 不是整数：{text}"))
}

fn access(pool: &std::sync::Arc<ChannelPool>, suffix: &str) -> McpAccess {
    let text = format!("lab=stdio:{}{suffix}", fixture());
    McpAccess::with_pool(
        parse_declaration(&text).expect("声明可以解析"),
        Vec::new(),
        std::sync::Arc::clone(pool),
    )
}

#[test]
fn one_channel_is_reused_across_runs_and_reaped_after_its_idle_lease() {
    let pool = ChannelPool::new(8, Duration::from_secs(30));
    let access = access(&pool, ",idle=1");

    let first = pid_of_one_run(access.clone());
    let second = pid_of_one_run(access.clone());
    assert_eq!(
        first, second,
        "两段 Run 应复用同一条通道——展开状态每段归零，通道不跟着重建"
    );

    std::thread::sleep(Duration::from_millis(1_200));
    pool.sweep();
    assert_eq!(pool.open_count(), 0, "空闲超过租期的那一台应被收掉");
    assert!(pool.reclaimed_count() >= 1, "回收要留读数");

    let third = pid_of_one_run(access);
    assert_ne!(first, third, "收掉之后下一次显式展开是新的子进程");
}

#[test]
fn a_keep_alive_channel_survives_any_idle_time() {
    let pool = ChannelPool::new(8, Duration::from_secs(1));
    let access = access(&pool, ",keep-alive");

    let first = pid_of_one_run(access.clone());
    std::thread::sleep(Duration::from_millis(1_200));
    pool.sweep();
    assert_eq!(pool.open_count(), 1, "keep-alive 不按时间收");
    assert_eq!(pool.reclaimed_count(), 0);

    let second = pid_of_one_run(access);
    assert_eq!(first, second, "keep-alive 的通道跨 Run 原样复用");
}

#[test]
fn removing_a_declaration_closes_its_channel() {
    let pool = ChannelPool::new(8, Duration::from_secs(30));
    let first = pid_of_one_run(access(&pool, ",keep-alive"));
    pool.retain(&[]);
    assert_eq!(pool.open_count(), 0, "这一段没有再声明它，通道当场收掉");

    // 下一段是**新的一份声明**（生产里 `declarations_for_run` 每段重建），不是复用旧 access。
    let second = pid_of_one_run(access(&pool, ",keep-alive"));
    assert_ne!(first, second, "下一段再声明它时是新进程");
}
