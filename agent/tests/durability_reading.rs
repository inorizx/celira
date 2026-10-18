//! **长跑读数：历史越长，打开与读数越慢吗**（手动跑，不进常规套件）。
//!
//! ## 为什么要有它
//!
//! 产品的验收测试是"**能被放着不管**"：开起来放着，过几天回来看，她还在、还记得、还会主动说话，
//! 中间没有人照看。而账本**只增不减**——打开、开场读数、健康自检都要读**整份已确认前缀**。
//! 真机读数里那份账本读到过"一次读 11.3 秒"（`host_loop` 的写资格耐心就是按它算出来的），
//! 但那一读数**混着等锁**：分不清是"解析慢"还是"排队慢"。这条用例把两件事分开量——
//! **同一个目录**上，写者路径与读者路径各计时一遍，并在三个规模上重复，好看出**斜率**
//! （线性可接受，超线性要查，常数太大也要查）。
//!
//! ## 它不设阈值
//!
//! **先量出来，再决定修什么**：阈值要等有了真机数字才定得下来。因此它是 `#[ignore]`，
//! 只打印读数；判据（"打开延迟不许随历史线性增长"这类上限）等这一读数落地之后再定。
//!
//! ## 跑法
//!
//! ```text
//! cargo test --offline --test durability_reading -- --ignored --nocapture
//! ```
//!
//! 合成历史用**真实写入 API** 生成（不是手写 NDJSON）：形状与真家一致，
//! 因此量的就是真家那条路。每个规模各用一个独立临时目录，跑完即删。

use std::path::{Path, PathBuf};
use std::time::Instant;

use celira_agent::core::{ModelOutput, ModelResponse};
use celira_agent::foundation::{AgentId, OperationId, Outcome, Record, RunId, SubjectEnding, Turn};
use celira_agent::store::{
    Environment, EnvironmentReader, TracePayload, ending_payload, response_payload,
};

/// 一个活动大致几步（与真机上的普通活动同量级）。
const TURNS_PER_RUN: u32 = 6;
/// 每条模型正文的字节数：真实活动里**正文是主要体积**（一次回复几百到几千字节）。
const CONTENT_BYTES: usize = 1_500;
/// 四个规模：**按 4 倍递增**，斜率才看得出来；最后一档（约 12 MB）**跨过封口线**（8 MiB）
/// ——好看出分卷之后"开机只读活动段 ＋ 目录"到底省了什么。
const SIZES: [usize; 4] = [20, 80, 320, 1200];

fn scratch(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("celira-durability-{}-{tag}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).expect("清掉同名残留");
    }
    std::fs::create_dir_all(&path).expect("建临时根");
    path
}

/// 生成 `runs` 个活动的合成历史；返回（经历字节数，事实字节数）。
fn generate(root: &Path, agent: &AgentId, runs: usize) -> (u64, u64) {
    let env_dir = root.join("env");
    let mut env = Environment::create(&env_dir, agent.clone()).expect("建环境");
    // 每字 3 字节（UTF-8），贴近真实中文正文的体积。
    let body = "字".repeat(CONTENT_BYTES / 3);
    for index in 0..runs {
        let run = RunId::new(format!("run-{index:05}"));
        env.append_trace(&run, TracePayload::RunStarted { origin: None })
            .expect("登记活动");
        env.append_fact(
            &run,
            &Record::RunStarted {
                agent: agent.clone(),
                run: run.clone(),
            },
        )
        .expect("登记事实");
        env.append_trace(
            &run,
            TracePayload::RunInput {
                text: format!("第 {index} 次醒来的输入"),
            },
        )
        .expect("写输入");
        for turn in 1..=TURNS_PER_RUN {
            let turn = Turn::from_number(turn).expect("轮次");
            let response = ModelResponse::new(turn, ModelOutput::say(body.clone()));
            env.append_trace(&run, response_payload(&response))
                .expect("写模型响应");
            let operation = OperationId::new(run.clone(), turn);
            env.append_fact(
                &run,
                &Record::ActionPrepared {
                    run: run.clone(),
                    operation: operation.clone(),
                },
            )
            .expect("写准备");
            env.append_fact(
                &run,
                &Record::ActionDispatched {
                    run: run.clone(),
                    operation,
                },
            )
            .expect("写派发");
        }
        env.append_trace(
            &run,
            ending_payload(Outcome::Subject(SubjectEnding::Completed)),
        )
        .expect("写收场");
        env.append_trace(&run, TracePayload::Sealed)
            .expect("写封存");
    }
    drop(env);
    let size = |name: &str| {
        std::fs::metadata(env_dir.join(name))
            .map(|meta| meta.len())
            .unwrap_or(0)
    };
    // ⚠ 经历那一列是**总量**（封口段 ＋ 活动段）：分卷之后只看 `trace.ndjson` 会少报一大截
    // （活动段封口后会重新变小）。事实那一列同一条规矩（它 2026-10-06 也分了卷）。
    let sealed_of = |name: &str| -> u64 {
        std::fs::read_dir(env_dir.join(name))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.metadata().map(|meta| meta.len()).unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0)
    };
    (
        sealed_of("trace.parts") + size("trace.ndjson"),
        sealed_of("facts.parts") + size("facts.ndjson"),
    )
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "为什么：要生成几十 MB 合成历史、跑几十秒，不进每次必跑的套件（它只打读数、不设阈值）；什么时候重跑：改了存储读法、账本形状或某格读数的合并方式之后，手动跑它看斜率"]
fn reading_latency_does_not_run_away_from_history_size() {
    println!(
        "\n活动 | 经历 MB | 事实 MB | 写者open | 开场读数 | 健康自检 | 读者open | 读者读数 | \
         读一段旧经历（最旧） | 读一段旧经历（中段）  (毫秒)"
    );
    let agent = AgentId::new("agent-durability");
    for runs in SIZES {
        let dir = scratch(&format!("n{runs}"));
        let (trace_bytes, fact_bytes) = generate(&dir, &agent, runs);
        let env_dir = dir.join("env");

        // 写者路径（取排他写资格，要求文件自洽）。
        let start = Instant::now();
        let env = Environment::open(&env_dir, agent.clone()).expect("写者打开");
        let open = ms(start);
        let start = Instant::now();
        let _ = env.opening_readings().expect("开场读数");
        let readings = ms(start);
        let start = Instant::now();
        let _ = env.health().expect("健康自检");
        let health = ms(start);
        drop(env);

        // 读者路径（不取写资格，读已确认前缀）。
        let start = Instant::now();
        let reader = EnvironmentReader::open(&env_dir, agent.clone()).expect("读者打开");
        let reader_open = ms(start);
        let start = Instant::now();
        let _ = reader.opening_readings().expect("读者开场读数");
        let reader_readings = ms(start);
        drop(reader);

        // **她自己读一段旧经历**：`files_read` 读 `history/<活动>/<段>.txt` 走的就是这条路
        // （`file_view` → `store::read_run_at`）。两列都是**最坏情况**的候选：
        // 「最旧」在第一卷里（要走卷内偏移索引 seek 过去），「中段」在一卷中间——两列都必须
        // **不随历史增长**，否则"卷内偏移索引"那一刀就没做到（它的判据见 README §4）。
        let start = Instant::now();
        let _ =
            celira_agent::store::read_run_at(&env_dir, &RunId::new("run-00000"), None, usize::MAX)
                .expect("读一段");
        let read_oldest = ms(start);
        let middle = RunId::new(format!("run-{:05}", runs / 2));
        let start = Instant::now();
        let _ =
            celira_agent::store::read_run_at(&env_dir, &middle, None, usize::MAX).expect("读一段");
        let read_middle = ms(start);

        println!(
            "{runs:>4} | {:>7.2} | {:>7.2} | {open:>8.1} | {readings:>8.1} | {health:>8.1} | \
             {reader_open:>8.1} | {reader_readings:>8.1} | {read_oldest:>18.1} | {read_middle:>18.1}",
            trace_bytes as f64 / 1e6,
            fact_bytes as f64 / 1e6
        );
        std::fs::remove_dir_all(&dir).expect("清掉临时根");
    }
    println!();
}
