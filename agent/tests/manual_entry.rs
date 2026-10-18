//! TASK_04 验收：**可实际使用的手动运行入口**。
//!
//! 这里验证的是"已经打通的环境与执行机制能由使用者通过明确入口使用，而且入口只负责接线与
//! 如实报告"。证据分成三层，各自只说自己那一层：
//!
//! - **真实子进程的生产命令**（`agent_run`）覆盖参数、帮助、初始化、标准输入与无网络拒绝路径；
//! - **真实子进程的离线夹具**（`manual_entry_fixture`）覆盖需要模型才能走到的路径。夹具与生产
//!   命令调用**同一段**启动代码、读同一份参数结构、用同一张退出码表，差别只在注入的模型；
//!   夹具与生产消费者的证据区别因此是"模型从哪来"，而不是"执行链是不是同一条"；
//! - **纯映射的单元断言**覆盖只需要查表就能判错的部分（退出码表）。
//!
//! 明确**不**声称的：
//!
//! - 没有运行任何公网模型：本文件里的模型要么是回环 HTTP 服务，要么是本地替身；
//! - 因此不宣称"真实模型能完成任务"，只宣称请求形状、手动来源与行动-结果往返是对的；
//! - 回环验证不等于真实模型效果验证。

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{CompositionError, SessionDeclarations};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{MAX_INPUT_BYTES, ModelFailureClass};
use celira_agent::file_app::FailureClass;
use celira_agent::foundation::{AgentId, RunId, SubjectEnding, SystemEnding};
use celira_agent::manual_entry::{
    EXIT_MODEL_ASSEMBLY_FAILED, EXIT_UNSETTLED, ManualEnding, ManualEntryError, ManualInput,
    ManualRun,
};
use celira_agent::store::Environment;
mod common;
use common::binary;

/// 出场正文：跨 Run 主动查历史时要能在旧活动里找到它。
const PROBE: &str = "第一次清醒期写下的合成正文（中文、含空格 与标点）";

// ---------------------------------------------------------------------------
// 1. 参数、帮助，以及"结论不依赖凭据是否存在"
// ---------------------------------------------------------------------------

#[test]
fn help_and_usage_errors_are_explicit_and_do_not_depend_on_a_credential() {
    let temp = TempDir::new("me-usage");
    let root = temp.path().join("env-never-created");
    let root_text = path_text(&root);
    let materials = temp.path().join("materials");

    // 帮助：不打任何东西，也不创建任何东西。
    let help = cli(&["--help"]);
    assert_eq!(code(&help), 0, "帮助应当成功退出");
    assert!(stdout(&help).contains("用法"), "{}", stdout(&help));
    assert!(
        stdout(&help).contains("--context-window"),
        "帮助必须写明窗口是宿主声明的: {}",
        stdout(&help)
    );
    assert!(!root.exists(), "帮助不得创建环境目录");

    // 参数错误：每一种都必须明确失败，而且都在**装配模型之前**。
    //
    // 凭据一律从子进程环境里摘掉，因此这一组断言证明的是——**这些路径的结论不依赖凭据是否存在**
    // （否则会退化成"凭据缺失"的退出码 5）。它**不**证明"代码里没有读凭据的调用"：读了却不依赖
    // 它，行为测试看不出来。那一条由**调用链审查**（静态、文本级）另行支持：装配模型的那次调用
    // 只出现在 `run` 的启动函数里（见 agent_run.rs 的顺序说明），这也是窄条件、不是链接期证明。
    let missing_subcommand = cli(&[]);
    assert_eq!(code(&missing_subcommand), 1);
    assert!(
        stderr(&missing_subcommand).contains("子命令"),
        "{}",
        stderr(&missing_subcommand)
    );

    let unknown_subcommand = cli(&["launch"]);
    assert_eq!(code(&unknown_subcommand), 1);

    let missing_agent = cli(&["init", "--root", &root_text]);
    assert_eq!(code(&missing_agent), 1, "init 缺 --agent");

    let missing_materials = cli(&["run", "--root", &root_text, "--agent", "agent-a"]);
    assert_eq!(code(&missing_materials), 1, "run 缺 --materials");

    let duplicated = cli(&[
        "run",
        "--root",
        &root_text,
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "2",
    ]);
    assert_eq!(code(&duplicated), 1, "同一个选项给两次必须失败");

    let unknown_option = cli(&[
        "run", "--root", &root_text, "--agent", "agent-a", "--nope", "x",
    ]);
    assert_eq!(code(&unknown_option), 1);

    let init_with_run_options = cli(&[
        "init",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--max-turns",
        "2",
    ]);
    assert_eq!(code(&init_with_run_options), 1, "init 不接受运行参数");

    let bad_turns = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "abc",
    ]);
    assert_eq!(code(&bad_turns), 1);

    let zero_turns = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "0",
    ]);
    assert_eq!(code(&zero_turns), 1, "Turn 上限必须是正整数");
    assert!(
        stderr(&zero_turns).contains("turn-budget-not-positive"),
        "{}",
        stderr(&zero_turns)
    );

    // TASK_19：上下文窗口也是**宿主声明**，`0` 不是"更小的窗口"而是一条无法解释的声明。
    // 拒绝发生在打开环境、装配模型之前（凭据已被摘掉，因此这个结论不依赖凭据）。
    let zero_window = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "2",
        "--context-window",
        "0",
    ]);
    assert_eq!(code(&zero_window), 1, "窗口必须是正整数");
    assert!(
        stderr(&zero_window).contains("context-window") && stderr(&zero_window).contains("正整数"),
        "{}",
        stderr(&zero_window)
    );

    let bad_window = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "2",
        "--context-window",
        "abc",
    ]);
    assert_eq!(code(&bad_window), 1, "窗口必须是整数");

    // init 不吃运行参数这条不变量同样覆盖它。
    let init_with_window = cli(&[
        "init",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--context-window",
        "100",
    ]);
    assert_eq!(code(&init_with_window), 1, "init 不接受运行参数");

    let conflicting_input = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "2",
        "--input",
        "正文",
        "--input-stdin",
    ]);
    assert_eq!(code(&conflicting_input), 1, "文本与标准输入互斥");

    // 环境不存在时**不会**自动创建：run 用既有 open，失败就是失败。
    let missing_environment = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "2",
    ]);
    assert_eq!(
        code(&missing_environment),
        2,
        "不存在的环境不得被自动初始化"
    );
    assert!(!root.exists(), "run 不得创建环境目录");

    // 帮助与参数错误都没有把结论打到 stdout。
    assert!(stdout(&missing_environment).is_empty());
}

// ---------------------------------------------------------------------------
// 2. 初始化
// ---------------------------------------------------------------------------

#[test]
fn init_is_explicit_repeating_it_is_refused_and_storage_is_untouched() {
    let temp = TempDir::new("me-init");
    let root = temp.path().join("env");
    let root_text = path_text(&root);
    let agent = AgentId::new("agent-init");

    let created = cli(&["init", "--root", &root_text, "--agent", "agent-init"]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));
    let report = stdout(&created);
    assert!(report.starts_with("celira-agent-run v1"), "{report}");
    assert_eq!(field(&report, "mode"), "init");
    assert_eq!(field(&report, "requested-agent"), "agent-init");
    assert_eq!(field(&report, "recorded-agent"), "agent-init");
    assert_eq!(field(&report, "status"), "created");

    // 后续 open 必须能校验身份：这正是"身份被真的记录下来"的证据。
    let opened = Environment::open(&root, agent.clone()).expect("身份必须可校验");
    assert_eq!(opened.agent(), &agent);
    drop(opened);

    let before = snapshot(&root);
    assert!(before.iter().any(|(name, _)| name == "identity.json"));

    // 重复初始化：拒绝，而且原存储一个字节都不变。
    let again = cli(&["init", "--root", &root_text, "--agent", "agent-init"]);
    assert_eq!(code(&again), 2, "已有环境必须被拒绝，不得重置");
    assert!(
        stderr(&again).contains("already-open"),
        "存储层自己的分类：有身份就是「打开」而不是「创建」，入口原样转述: {}",
        stderr(&again)
    );
    assert!(stdout(&again).is_empty());
    assert_eq!(snapshot(&root), before, "重复 init 不得改动原存储");

    // 换一个身份：同样拒绝（不覆盖、不重置）。
    let other = cli(&["init", "--root", &root_text, "--agent", "agent-other"]);
    assert_eq!(code(&other), 2);
    assert_eq!(snapshot(&root), before);
}

// ---------------------------------------------------------------------------
// 3. 输入边界
// ---------------------------------------------------------------------------

#[test]
fn input_sources_are_explicit_bounded_and_never_truncated() {
    let temp = TempDir::new("me-input");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");

    let run_with = |input: ManualInput| ManualRun {
        declarations: SessionDeclarations::default(),
        environment_root: root.clone(),
        expected_agent: AgentId::new("agent-input"),
        material_root: materials.clone(),
        run: RunId::new("run-input"),
        max_turns: 2,
        input,
        // 用例装配用的时间来源：固定钟（离线、可逐字节断言）；生产命令给真实钟。
        clock: ClockHandle::fixed_at_unix_seconds(0),
        // 宿主声明的窗口：用例显式声明一个小数字（生产缺省用适配层的公开标称值）。
        context_window: 1_000,
        // 上下文占用那一格的读侧：空 cell ⇒ 状态行读作 `context = unknown/1000 token`。
        reading: ContextReading::default(),
    };

    let resolve = |input: ManualInput, bytes: &[u8]| {
        let run = run_with(input);
        let mut reader = bytes;
        run.resolve_input(&mut reader)
    };

    // 空输入合法：不给内容也是一次正常活动。
    assert_eq!(resolve(ManualInput::Empty, b"").unwrap().as_str(), "");
    assert_eq!(resolve(ManualInput::Stdin, b"").unwrap().as_str(), "");

    // 中文 UTF-8 原样保留，一个字节都不动。
    let chinese = "中文正文与标点，含换行\n第二行";
    let resolved = resolve(ManualInput::Stdin, chinese.as_bytes()).expect("应当接受");
    assert_eq!(resolved.as_str(), chinese);
    assert_eq!(resolved.len(), chinese.len());

    // 正好到上限：接受。
    let at_limit = vec![b'a'; MAX_INPUT_BYTES];
    let resolved = resolve(ManualInput::Stdin, &at_limit).expect("上限内应当接受");
    assert_eq!(resolved.len(), MAX_INPUT_BYTES);

    // 超一个字节：拒绝，而且**不截断**成一份看起来完整的输入。
    let over_limit = vec![b'a'; MAX_INPUT_BYTES + 1];
    assert_eq!(
        resolve(ManualInput::Stdin, &over_limit),
        Err(ManualEntryError::InputTooLarge {
            limit: MAX_INPUT_BYTES
        })
    );
    assert_eq!(
        resolve(ManualInput::Text("a".repeat(MAX_INPUT_BYTES + 1)), b""),
        Err(ManualEntryError::InputTooLarge {
            limit: MAX_INPUT_BYTES
        })
    );

    // 非法编码：明确失败，不猜。
    assert_eq!(
        resolve(ManualInput::Stdin, &[0xff, 0xfe, 0xfd]),
        Err(ManualEntryError::InputNotUtf8)
    );

    // Turn 上限不是正整数：拒绝。
    let mut zero = run_with(ManualInput::Empty);
    zero.max_turns = 0;
    assert_eq!(
        zero.check(),
        Err(ManualEntryError::TurnBudgetNotPositive { max_turns: 0 })
    );

    // 被拒绝的输入不得启动活动：环境目录一个都没被创建。
    assert!(!root.exists(), "入口拒绝时不得创建或打开环境");

    // 真实子进程：超限的标准输入同样是退出码 1，而且环境仍未被创建。
    let oversized = cli_with_stdin(
        &[
            "run",
            "--root",
            &path_text(&root),
            "--agent",
            "agent-input",
            "--materials",
            &path_text(&materials),
            "--run",
            "run-input",
            "--max-turns",
            "2",
            "--input-stdin",
        ],
        &vec![b'a'; MAX_INPUT_BYTES + 1],
    );
    assert_eq!(code(&oversized), 1, "超限的标准输入必须明确失败");
    assert!(
        stderr(&oversized).contains("input-too-large"),
        "{}",
        stderr(&oversized)
    );
    assert!(!root.exists(), "输入被拒绝时不得创建环境");

    // 在一个**真实存在**的环境上再验一次：输入被拒绝时活动根本不会启动——
    // "环境不存在"证明不了这一条，因为那时本来也没有东西可以启动。
    let real = temp.path().join("env-real");
    let real_materials = temp.path().join("materials-real");
    std::fs::create_dir_all(&real_materials).expect("建资料目录");
    let created = cli(&[
        "init",
        "--root",
        &path_text(&real),
        "--agent",
        "agent-input",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));
    let before = snapshot(&real);
    let refused_on_real = cli_with_stdin(
        &[
            "run",
            "--root",
            &path_text(&real),
            "--agent",
            "agent-input",
            "--materials",
            &path_text(&real_materials),
            "--run",
            "run-input",
            "--max-turns",
            "2",
            "--input-stdin",
        ],
        &vec![b'a'; MAX_INPUT_BYTES + 1],
    );
    assert_eq!(code(&refused_on_real), 1);
    assert_eq!(
        snapshot(&real),
        before,
        "输入被拒绝时不得启动活动：环境一个字节都不许变"
    );
    assert!(
        !std::fs::read_to_string(real.join("facts.ndjson"))
            .expect("读事实")
            .contains("run-input"),
        "被拒绝的输入不得为这次活动留下任何事实"
    );

    // 非法编码的标准输入：同样是入口拒绝。
    let invalid = cli_with_stdin(
        &[
            "run",
            "--root",
            &path_text(&root),
            "--agent",
            "agent-input",
            "--materials",
            &path_text(&materials),
            "--run",
            "run-input",
            "--max-turns",
            "2",
            "--input-stdin",
        ],
        &[0xff, 0xfe],
    );
    assert_eq!(code(&invalid), 1);
    assert!(
        stderr(&invalid).contains("input-not-utf8"),
        "{}",
        stderr(&invalid)
    );
}

// ---------------------------------------------------------------------------
// 4. 多 Run：第二次 Run 主动读回第一次
// ---------------------------------------------------------------------------

#[test]
fn two_runs_share_one_environment_and_the_second_reads_history_on_purpose() {
    let temp = TempDir::new("me-multi");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    std::fs::write(materials.join("note.txt"), "资料正文").expect("写资料");

    let created = cli(&[
        "init",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-multi",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    // A：经共用生产启动路径保存一段带正文的合成经历，然后退出。
    let a = Fixture::new(&root, "agent-multi", &materials, "run-a", "script")
        .max_turns(2)
        .script("finish")
        .input(PROBE)
        .run();
    assert_eq!(code(&a), 0, "{}", stderr(&a));
    assert_eq!(field(&stdout(&a), "ending"), "subject:ended");
    assert_eq!(fixture(&stdout(&a), "model-calls"), "1");

    // B：独立进程打开同一环境，开新的一次活动，**主动**经通用文件能力去查 A。
    let b = Fixture::new(&root, "agent-multi", &materials, "run-b", "read-back")
        // TASK_69：B 要的轮数多了一轮——开场那批"先看格式"占掉一轮，之后展开又带出第二批。
        // 六拍会被预算顶掉，收场变成 `system:timeout`（退出码 7），这个舞台就验不到它要验的东西。
        .max_turns(7)
        .input("B 的初始输入")
        .probe(PROBE)
        .run();
    assert_eq!(code(&b), 0, "{}", stderr(&b));
    let report = stdout(&b);
    assert_eq!(field(&report, "ending"), "subject:ended");
    assert_eq!(fixture(&report, "first-view-had-probe"), "false");
    assert_eq!(
        fixture(&report, "first-view-had-foreign-actions"),
        "false",
        "B 第一次清醒、还没做任何（业务）事时，视野里不得有从别处搬来的行动结果"
    );
    assert_eq!(fixture(&report, "saw-list"), "true");
    assert_eq!(fixture(&report, "saw-read"), "true");
    assert_eq!(
        fixture(&report, "read-back-had-probe"),
        "true",
        "B 必须真的从通用文件能力里读回 A 留下的正文"
    );
    assert_eq!(
        fixture(&report, "observations-match-own-actions"),
        "true",
        "B 的观察条数必须正好等于它自己发出的动作数：不自动重放 A 的行动"
    );
}

// ---------------------------------------------------------------------------
// 5. 真实适配层 + 回环服务
// ---------------------------------------------------------------------------

#[test]
fn a_real_loopback_request_carries_the_manual_origin_and_round_trips_an_action() {
    let temp = TempDir::new("me-loopback");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    let created = cli(&["init", "--root", &path_text(&root), "--agent", "agent-wire"]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    // 回环服务：第一轮答"展开文件视图"，第二轮答"收场"。两轮的请求体都被记下来。
    let (endpoint, requests) = spawn_reply_server();

    let run = Fixture::new(&root, "agent-wire", &materials, "run-wire", "loopback")
        .max_turns(4)
        .input(PROBE)
        .endpoint(&endpoint, "fixture-sentinel-credential")
        .run();

    let report = stdout(&run);
    let diagnostics = stderr(&run);
    assert_eq!(code(&run), 0, "stdout={report} stderr={diagnostics}");
    assert_eq!(field(&report, "ending"), "subject:ended");
    assert_eq!(fixture(&report, "model-calls"), "2");
    // 哨兵凭据不得出现在任何输出里。
    assert!(!report.contains("fixture-sentinel-credential"));
    assert!(!diagnostics.contains("fixture-sentinel-credential"));

    let bodies = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("回环服务应当收到两轮请求");
    let (first, second) = bodies
        .split_once("\n=====REQUEST=====\n")
        .expect("两轮请求");

    // 第一轮：system 契约在最前，开场正文以 user 正文出现且**恰好一次**；手动来源作为
    // 一条环境内容消息出现，而**不**被升成 system 契约。
    assert!(first.contains("\"role\":\"system\""), "{first}");
    assert!(first.contains("\"role\":\"user\""), "{first}");
    assert_eq!(
        first.matches(PROBE).count(),
        1,
        "被接受的开场输入必须在首个请求里原样且一次出现"
    );
    assert!(
        first.contains("EXTERNAL DATA (produced by the environment, not by you) [activity start]"),
        "手动发起这条环境内容必须出现在请求里: {first}"
    );
    let first_system = first
        .split("\"role\":\"user\"")
        .next()
        .expect("有 system 段");
    assert!(
        !first_system.contains(PROBE),
        "开场正文不得进入 system 契约"
    );

    // 第二轮：上一轮行动的结果以**原生工具回执**的形状回到请求里（行动 → 结果 → 再请求这条
    // 往返成立），它与那次请求的调用标识相邻配对；而且展开之后能力目录**真的**多出了文件能力。
    assert!(
        second.contains("\"role\":\"tool\"") && second.contains("\"call-expand\""),
        "第二轮必须带上上一轮行动的回执，并与调用标识配对: {second}"
    );
    assert!(
        second.contains("action = open_app") && second.contains("state = success"),
        "回执里必须认得是哪一个行动、以及真实结果: {second}"
    );
    // 头部**恒定两件**（TASK_68）：展开不再改 tools 前缀——能力改由入口 `invoke` 路由，
    // 展开的效果走**注入的目录**（P2）。这一格钉的是"头部没变"。
    assert!(
        second.contains("\"name\":\"invoke\"") && second.contains("\"name\":\"finish_run\""),
        "展开之后头部仍是那两件: {second}"
    );
    assert!(
        !second.contains("\"name\":\"files_list\""),
        "app 动词不再直连头部: {second}"
    );
    let first_system = first
        .split("\"role\":\"user\"")
        .next()
        .expect("有 system 段");
    assert!(
        !first_system.contains("files_read"),
        "还没展开时契约里不该有文件能力: {first_system}"
    );
}

/// 四轮脚本：展开材料窗 → 读那份材料 → 做一件**不读它**的事 → 收场。
///
/// 第三轮那件"不读它的事"是判据本身：**只有在一轮之后仍然看得到**，才叫"留在 Run 里"。
const RETAIN_EXPAND: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-expand","type":"function","function":{"name":"open_app","arguments":"{\"target\":\"files\"}"}}]},"finish_reason":"tool_calls"}]}"#;
/// TASK_69：调用之前**先看格式**——没查过的动作在准入面就被拒（拒绝的语义是"先看格式"）。
const RETAIN_SHAPE: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-shape","type":"function","function":{"name":"inspect_action","arguments":"{\"action\":\"files_read\"}"}}]},"finish_reason":"tool_calls"}]}"#;
const RETAIN_READ: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-read","type":"function","function":{"name":"files_read","arguments":"{\"path\":\"probe.txt\"}"}}]},"finish_reason":"tool_calls"}]}"#;
const RETAIN_ELSEWHERE: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-elsewhere","type":"function","function":{"name":"inspect_action","arguments":"{\"action\":\"files_list\"}"}}]},"finish_reason":"tool_calls"}]}"#;
const RETAIN_FINISH: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish_run","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

/// **Run 内忠实保留**（TASK_66 的钉子）：一个 Run 里**读到的东西，后面的轮次照样看得到**。
///
/// 判错即红：把 `Run::view()` 的轨迹截成"只留最近一轮"，这条用例立刻红。也就是说，谁想把
/// 「轮末即弃」写回实现，谁就会在这里被当场拦住——这是本刀**唯一**的机制价值（现状本来就是对的，
/// 缺的只是守卫与措辞）。
///
/// 轮次是刻意摆的：**读发生在第 3 轮**（第 1 轮只是展开材料窗，第 2 轮是"先看格式"——TASK_69
/// 之后没查过的动作在准入面就被拒），所以"内容出现"落在第 4 轮请求里；
/// 而**第 5 轮请求**才是判据的重点——那一轮的应答做的是**另一件事**（看另一张动作卡片），
/// 主体**没有再读**那份文件，内容却仍在视野里。这才是"读过的东西留在主体的 Run 里"的真身。
#[test]
fn what_she_read_stays_visible_in_the_later_turns_of_the_same_run() {
    const NONCE: &str = "RUN-SCOPED-PROBE-9F4C";
    let temp = TempDir::new("me-run-retention");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    std::fs::write(materials.join("probe.txt"), format!("{NONCE}\n")).expect("放材料");
    let created = cli(&[
        "init",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-retain",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    let (endpoint, requests) = spawn_scripted_server(vec![
        RETAIN_EXPAND,
        RETAIN_SHAPE,
        RETAIN_READ,
        RETAIN_ELSEWHERE,
        RETAIN_FINISH,
    ]);
    let run = Fixture::new(&root, "agent-retain", &materials, "run-retain", "loopback")
        .max_turns(6)
        .input("把资料里那份 probe.txt 读出来")
        .endpoint(&endpoint, "fixture-sentinel-credential")
        .run();

    let report = stdout(&run);
    assert_eq!(code(&run), 0, "stdout={report}\nstderr={}", stderr(&run));
    assert_eq!(
        fixture(&report, "model-calls"),
        "5",
        "展开、先看格式、读、另一件事、收场各一轮"
    );

    let bodies = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("五轮请求");
    // ⚠️ 装置把**全部**请求体合并后一次交出 ⇒ 必须按分隔符切成"第几轮"，
    // 否则断言会落在"合并体"上（那样"哪一轮看得到"就分不出来了）。
    let rounds: Vec<&str> = bodies.split("\n=====REQUEST=====\n").collect();
    assert_eq!(rounds.len(), 5, "五轮各一份请求");

    // ① 第 1 轮：只是展开，还没读 ⇒ 内容不该在场。
    assert!(!rounds[0].contains(NONCE), "第 1 轮还没读: {}", rounds[0]);
    // ② 第 2 轮：只是"先看格式"，也没读 ⇒ 仍然不在场。
    assert!(
        !rounds[1].contains(NONCE),
        "第 2 轮只是看格式，那次读还没发生: {}",
        rounds[1]
    );
    // ③ 第 3 轮：读还没发生（这一轮的应答才是"去读"）⇒ 仍然不在场。
    assert!(
        !rounds[2].contains(NONCE),
        "第 3 轮时那次读还没执行: {}",
        rounds[2]
    );
    // ④ 第 4 轮：刚读到 ⇒ 结果在（"当轮"那一格）。
    assert!(
        rounds[3].contains(NONCE),
        "第 4 轮该带着刚读到的内容: {}",
        rounds[3]
    );
    // ⑤ 第 5 轮：主体这一轮做的是**另一件事**，没有再读，内容仍在 —— 本刀要钉住的正是这一格。
    assert!(
        rounds[4].contains(NONCE),
        "第 5 轮不再读也看得到（Run 内忠实保留）: {}",
        rounds[4]
    );
    assert!(
        rounds[4].contains("action = files_read") && rounds[4].contains("\"role\":\"tool\""),
        "它的载体是那次读的结果回执: {}",
        rounds[4]
    );
    assert_eq!(
        rounds[3].matches("action = files_read").count(),
        1,
        "第 4 轮视野里那次读**恰好一份**（没有重读、也没有复制）"
    );
}

/// 同进程宿主输入循环：显式输入恰好一次、纯正文等待、EOF 如实报成"活动没结束"。
///
/// 这里走的是**生产会话接口**（夹具消费的就是 `manual_entry` 那个宿主循环，与生产命令同一条
/// 启动代码与同一张退出码表），不是另造的执行路径。
#[test]
fn the_host_input_loop_takes_explicit_input_and_reports_eof_as_waiting() {
    const TEXT: &str = r#"{"choices":[{"message":{"role":"assistant","content":"我先说一句，等宿主给下一段"},"finish_reason":"stop"}]}"#;
    const FINISH: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish_run","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

    // (1) 一行非空文本 = 显式输入：纯正文那一轮停下等待，之后由这一行接着驱动到收场。
    let temp = TempDir::new("me-host");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    let created = cli(&["init", "--root", &path_text(&root), "--agent", "agent-host"]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    let (endpoint, requests) = spawn_scripted_server(vec![TEXT, FINISH]);
    let run = Fixture::new(&root, "agent-host", &materials, "run-host", "loopback")
        .max_turns(4)
        .input(PROBE)
        .endpoint(&endpoint, "fixture-sentinel-credential")
        .host_lines("宿主第二段\n不该被读到的第三段\n")
        .run();
    let report = stdout(&run);
    assert_eq!(code(&run), 0, "stdout={report} stderr={}", stderr(&run));
    assert_eq!(field(&report, "ending"), "subject:ended");
    assert_eq!(fixture(&report, "model-calls"), "2");
    assert!(
        !report.contains("fixture-sentinel-credential")
            && !stderr(&run).contains("fixture-sentinel-credential"),
        "哨兵凭据不得出现在任何输出里"
    );

    let bodies = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("回环服务应当收到两轮请求");
    let (first, second) = bodies
        .split_once("\n=====REQUEST=====\n")
        .expect("两轮请求");
    assert!(
        !first.contains("宿主第二段"),
        "开场轮不得带上还没给的输入: {first}"
    );
    assert_eq!(
        second.matches("宿主第二段").count(),
        1,
        "显式输入必须在它启用的那一轮里恰好出现一次"
    );
    assert!(
        second.contains("我先说一句，等宿主给下一段"),
        "上一轮正文按协议作为历史回传"
    );
    assert!(
        !second.contains("不该被读到的第三段"),
        "收场之后宿主循环不得再读下一行"
    );

    // (2) 流结束 = 显式退出：停在等待，退出码 9，而且零额外请求。
    let eof_root = temp.path().join("env-eof");
    let eof_materials = temp.path().join("materials-eof");
    std::fs::create_dir_all(&eof_materials).expect("建资料目录");
    let created = cli(&[
        "init",
        "--root",
        &path_text(&eof_root),
        "--agent",
        "agent-host",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    let (endpoint, requests) = spawn_scripted_server(vec![TEXT]);
    let eof = Fixture::new(
        &eof_root,
        "agent-host",
        &eof_materials,
        "run-eof",
        "loopback",
    )
    .max_turns(4)
    .input(PROBE)
    .endpoint(&endpoint, "fixture-sentinel-credential")
    .host_lines("")
    .run();
    let report = stdout(&eof);
    assert_eq!(
        code(&eof),
        i32::from(EXIT_UNSETTLED),
        "停在等待必须有自己的退出码\nstdout={report}\nstderr={}",
        stderr(&eof)
    );
    assert_eq!(field(&report, "ending"), "waiting");
    assert_eq!(
        fixture(&report, "model-calls"),
        "1",
        "EOF 之后不得再驱动一次"
    );
    // 报告必须说清后果：活动没有结束，环境里留着一个未收尾的活动。
    assert!(
        report.contains("未收尾的活动"),
        "报告要说明环境里留下了什么: {report}"
    );
    assert!(
        report.contains("本入口不自动续跑"),
        "报告不得暗示自动续跑: {report}"
    );
    let bodies = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("回环服务应当收到一轮请求");
    assert_eq!(
        bodies.matches("=====REQUEST=====").count(),
        0,
        "只应有一轮请求"
    );
    // 后果留在环境里：但那**不再拦住**同一个环境的下一次活动（2026-09-26 新规矩）。
    //
    // ⚠ **旧规矩**：这里原来断言 `code == 3` 且 `model-calls == 0`——"未收尾准入把它挡住"。
    // **新规矩**：进程没了就是那一段断了，新进程起来是**新的一段** ⇒ 放行、跑起来。
    // **放行不等于解决**：那一段的未收尾痕迹仍然在盘上（由 `durable_environment.rs` 守着）。
    let proceeded = Fixture::new(
        &eof_root,
        "agent-host",
        &eof_materials,
        "run-next",
        "counting",
    )
    .max_turns(2)
    .run();
    assert_eq!(code(&proceeded), 0, "{}", stdout(&proceeded));
}

// ---------------------------------------------------------------------------
// 6. 拒绝：零模型请求、零动作、不伪造新事实
// ---------------------------------------------------------------------------

#[test]
fn refusals_never_reach_the_model_and_never_fabricate_facts() {
    let temp = TempDir::new("me-refuse");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");

    let created = cli(&[
        "init",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-refuse",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    // (1) 错身份：既有 open 校验归属，拒绝发生在建立执行对象之前。
    let wrong_identity = Fixture::new(&root, "agent-other", &materials, "run-1", "counting")
        .max_turns(2)
        .run();
    assert_eq!(code(&wrong_identity), 2, "错身份必须被拒绝");
    assert_eq!(fixture(&stdout(&wrong_identity), "model-calls"), "0");
    assert!(!stdout(&wrong_identity).contains("ending=subject"));

    // (2) 写锁冲突：另一个句柄仍持有时必须被拒绝。
    let held = Environment::open(&root, AgentId::new("agent-refuse")).expect("先取得写资格");
    let second_writer = Fixture::new(&root, "agent-refuse", &materials, "run-1", "counting")
        .max_turns(2)
        .run();
    assert_eq!(code(&second_writer), 2, "第二个写者必须被拒绝");
    assert_eq!(fixture(&stdout(&second_writer), "model-calls"), "0");
    drop(held);

    // (3) 资料根与环境根重叠：装配期拒绝，仍然排在模型调用之前。
    let overlap = Fixture::new(&root, "agent-refuse", &root, "run-1", "counting")
        .max_turns(2)
        .run();
    assert_eq!(code(&overlap), 4, "资料根覆盖环境内部记录必须被拒绝");
    assert_eq!(fixture(&stdout(&overlap), "model-calls"), "0");

    // 先正常跑一次，为后面的重复活动标识与暂停做铺垫。
    let first = Fixture::new(&root, "agent-refuse", &materials, "run-1", "script")
        .max_turns(2)
        .script("finish")
        .run();
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let facts_before = std::fs::read_to_string(root.join("facts.ndjson")).expect("读事实");

    // (4) 重复活动标识：既有确认链在第一次模型调用之前拒绝；这是保存失败，不是"成功"。
    let duplicate = Fixture::new(&root, "agent-refuse", &materials, "run-1", "script")
        .max_turns(2)
        .script("act:open_app?target=files;finish")
        .run();
    assert_eq!(code(&duplicate), 6, "重复活动标识必须报成保存未完整确认");
    let duplicate_report = stdout(&duplicate);
    assert!(
        duplicate_report.contains("refused=save-failed"),
        "{duplicate_report}"
    );
    assert!(
        duplicate_report.contains("save-failed stage=run-start"),
        "{duplicate_report}"
    );
    assert!(
        duplicate_report.contains("ending-confirmed=false"),
        "终态没有被确认，报告必须这么说: {duplicate_report}"
    );
    assert!(
        !duplicate_report.contains("成功"),
        "保存失败不得被说成成功: {duplicate_report}"
    );
    assert_eq!(
        fixture(&duplicate_report, "model-calls"),
        "0",
        "该被拒绝时一次模型都不许被叫到"
    );
    // 新事实没有被伪造：这次失败没有为同一个活动写第二条开始事实。
    let facts_after = std::fs::read_to_string(root.join("facts.ndjson")).expect("读事实");
    let started_for_run: Vec<&str> = facts_after
        .lines()
        .filter(|line| line.contains("\"run\":\"run-1\"") && line.contains("run-started"))
        .collect();
    assert_eq!(started_for_run.len(), 1, "同一个活动只能有一条开始事实");
    assert!(
        facts_after.starts_with(&facts_before),
        "失败不得改写已经确认的事实前缀"
    );

    // (5) 暂停：上一步留下了已记录的保存故障——**不再拒绝**（2026-10-06 新规矩，主人裁决）。
    //
    // ⚠ **旧规矩**：这里断言 `code == 3` + `refused=paused` + 零模型调用（"重开不能绕过它"）。
    // **新规矩**：**门是指令，而世界只该给信息**——那条故障是**事实**，由开场那一块纸条带给
    // 主体（`PreviousActivity` 的 `StorageFailure` 那一段），能不能继续由她看着事实自己定。
    // ⇒ 照常跑（模型真的被调用）；而"**不得把它说成别的**"那半句一个字没松：那条记录照旧在
    // 准入读数里（`agent_host_status` 那一侧）与开场纸条里。
    let paused = Fixture::new(&root, "agent-refuse", &materials, "run-2", "counting")
        .max_turns(2)
        .run();
    assert_eq!(code(&paused), 0, "暂停不再是闸：照常跑完");
    assert_ne!(
        fixture(&stdout(&paused), "model-calls"),
        "0",
        "它真的跑了：模型被调用"
    );
    assert!(
        !stdout(&paused).contains("refused=paused"),
        "那条拒绝读数随闸一起退役: {}",
        stdout(&paused)
    );

    // (6) 没有收尾的活动：**不再拒绝**（2026-09-26 新规矩）。
    //
    // ⚠ **旧规矩**：这里原来断言 `code == 3` + `refused=unfinished` + 点名 `run-unfinished`。
    // **新规矩**：进程没了就是那一段断了，新进程起来是**新的一段** ⇒ 放行、跑起来。
    // 而"**不得把它说成存储故障**"那半句一个字没松——它由上面 (5) 的 `paused` 守着：
    // **真正的**存储故障仍然如实拒绝、仍然零模型调用。
    let unfinished_root = temp.path().join("unfinished");
    let seeded = spawn_worker("leave-unfinished", &unfinished_root);
    assert!(seeded.status.success(), "夹具应当建立未收尾环境");
    let unfinished = Fixture::new(
        &unfinished_root.join("env"),
        "agent-alpha",
        &materials,
        "run-new",
        "counting",
    )
    .max_turns(2)
    .run();
    assert_eq!(
        code(&unfinished),
        0,
        "未收尾的活动不再拒绝新活动：{}",
        stdout(&unfinished)
    );
    assert!(
        !stdout(&unfinished).contains("refused=paused"),
        "它**不是**存储故障，因此不得报成 paused"
    );

    // (7) 用**真实适配层 + 回环端点**再验一次"零模型请求"：计数替身只能说明模型没被叫到，
    // 回环服务能说明这一刻**一个字节都没有出网**。
    let (endpoint, listener) = spawn_silent_endpoint();
    let over_the_wire = Fixture::new(&root, "agent-other", &materials, "run-wire", "loopback")
        .max_turns(2)
        .endpoint(&endpoint, "fixture-sentinel-credential")
        .run();
    assert_eq!(code(&over_the_wire), 2, "错身份必须被拒绝");
    assert!(
        stderr(&over_the_wire).contains("identity-mismatch"),
        "拒绝原因必须来自存储层自己的分类: {}",
        stderr(&over_the_wire)
    );
    assert_no_request(&listener);
}

// ---------------------------------------------------------------------------
// 7. 收场与退出码
// ---------------------------------------------------------------------------

#[test]
fn every_ending_keeps_its_own_category_and_exit_code() {
    let temp = TempDir::new("me-ending");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    let created = cli(&[
        "init",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-ending",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    let subject = |script: &str, run: &str, turns: u32| {
        Fixture::new(&root, "agent-ending", &materials, run, "script")
            .max_turns(turns)
            .script(script)
            .run()
    };

    // 主体收场：**只有一档**（2026-10-10 裁决——收场不带值，结束了就是结束了）。
    // 从前这里逐项验 `silent` / `waiting` 各自的类别；那两个值线缆上已经产不出来，
    // 于是这一条改成验"她收场就是 `subject:ended`，而且**不被说成任务完成**"。
    let ended = subject("finish", "run-ended", 2);
    assert_eq!(code(&ended), 0);
    assert_eq!(field(&stdout(&ended), "ending"), "subject:ended");

    // 模型失败：脚本用尽 → 系统终止，不是主体收场。
    let model_failed = subject("", "run-model-failed", 2);
    assert_eq!(code(&model_failed), 7, "模型失败必须是系统终止");
    assert_eq!(
        field(&stdout(&model_failed), "ending"),
        "system:model-failed"
    );

    // Turn 用尽：行动一直成功，但从没收场 → 系统终止，不冒充完成。
    let budget = subject("act:open_app?target=files", "run-budget", 1);
    assert_eq!(code(&budget), 7, "Turn 用尽必须是系统终止");
    assert_eq!(field(&stdout(&budget), "ending"), "system:timeout");

    // 纯映射：不需要模型就能判错的那部分，逐项查表。
    assert_eq!(
        ManualEnding::Subject(SubjectEnding::Completed).exit_code(),
        0
    );
    assert_eq!(
        ManualEnding::Subject(SubjectEnding::Declined).exit_code(),
        0
    );
    for ending in [
        // 唯一带载荷的一格：两种形态都要落在同一个退出码上（载荷不改变行为）。
        SystemEnding::ModelFailed(None),
        SystemEnding::ModelFailed(Some(ModelFailureClass::ContractViolation.as_str())),
        SystemEnding::ActionFailed,
        SystemEnding::Cancelled,
        SystemEnding::Timeout,
        SystemEnding::Crashed,
        SystemEnding::Indeterminate,
    ] {
        assert_eq!(
            ManualEnding::System(ending).exit_code(),
            7,
            "{ending:?} 必须是系统终止那一类"
        );
    }
    // 等待**不是**一个结局：它有自己的一格，与"主体自己收场"（0）明确分开。
    assert_eq!(ManualEnding::Unsettled.exit_code(), EXIT_UNSETTLED);
    assert_ne!(ManualEnding::Unsettled.exit_code(), 0);
    // 主体的 `waiting` 收场是另一件事：那是主体说"我在等外部的什么"并因此结束了活动。
    assert_eq!(ManualEnding::Subject(SubjectEnding::Waiting).exit_code(), 0);
    assert_ne!(
        ManualEnding::Subject(SubjectEnding::Waiting).exit_code(),
        ManualEnding::Unsettled.exit_code()
    );
    assert_eq!(
        ManualEnding::Refused(CompositionError::SessionNotDrivable).exit_code(),
        4
    );
    assert_eq!(
        ManualEnding::Refused(CompositionError::Environment("x".into())).exit_code(),
        2
    );
    // ⚠ **2026-09-26 删**：`UnfinishedRun` 那一条退出码断言随变体一起退役 ——
    // "没有收尾的旧活动"不再是**拒绝**（进程没了就是那一段断了，新进程起来是新的一段）。
    // ⚠ **2026-10-06 删**：`StoragePaused` 那一条（码 `3`）随那道闸一起退役 ——
    // 已记录在案的保存失败也**不再是拒绝**（它是**事实**，由开场纸条带给主体，见 §3.3）。
    // ⇒ 今天**没有"需要你动手"这一档**：装配失败一律归 `2`。
    assert_eq!(
        ManualEnding::Refused(CompositionError::FileAppRefused(FailureClass::NotFound)).exit_code(),
        4
    );
    assert_eq!(
        ManualEnding::Refused(CompositionError::MachineRefused("x".into())).exit_code(),
        4
    );
    assert_eq!(EXIT_MODEL_ASSEMBLY_FAILED, 5);
    assert_eq!(
        ManualEnding::Refused(CompositionError::SaveFailed {
            stage: celira_agent::core::CommitStage::Ending,
            ending_confirmed: false,
            pause_recorded: true,
        })
        .exit_code(),
        6
    );
}

// ---------------------------------------------------------------------------
// 8. 保存故障
// ---------------------------------------------------------------------------

#[test]
fn a_save_fault_is_reported_as_unconfirmed_and_never_as_success() {
    let temp = TempDir::new("me-save-fault");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    let created = cli(&[
        "init",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-fault",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    let script = "act:open_app?target=files;finish";
    let first = Fixture::new(&root, "agent-fault", &materials, "run-1", "script")
        .max_turns(3)
        .script(script)
        .run();
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let second = Fixture::new(&root, "agent-fault", &materials, "run-2", "script")
        .max_turns(3)
        .script(script)
        .run();
    assert_eq!(code(&second), 0, "{}", stderr(&second));

    // 用一个已经用过的活动标识再跑一次：这是**经生产路径**得到的真实保存故障。
    let duplicated = Fixture::new(&root, "agent-fault", &materials, "run-1", "script")
        .max_turns(3)
        .script(script)
        .run();
    assert_eq!(code(&duplicated), 6, "保存未完整确认必须有自己的退出码");
    let report = stdout(&duplicated);
    assert!(report.contains("refused=save-failed"), "{report}");
    assert!(report.contains("save-failed stage=run-start"), "{report}");
    assert!(report.contains("ending-confirmed=false"), "{report}");
    assert!(report.contains("pause-recorded=true"), "{report}");
    // 只报告当前调用能证明的边界，不声称成功、不声称记录不存在、不声称行动被撤销。
    assert!(report.contains("不重发已发出的动作"), "{report}");
    assert!(!report.contains("成功"), "{report}");
    assert_eq!(
        fixture(&report, "model-calls"),
        "0",
        "保存失败不得触发重发或补一次模型调用"
    );

    // 故障确实被记下来了——**但它不再拦下一次活动**（2026-10-06 新规矩，主人裁决）：
    // 那条记录是**事实**（由开场那一块纸条带给主体），能不能继续由她看着事实自己定。
    // ⚠ **旧规矩**：这里断言 `code == 3` + 零模型调用（"重开不能绕过它"）。
    let after = Fixture::new(&root, "agent-fault", &materials, "run-3", "counting")
        .max_turns(2)
        .run();
    assert_eq!(code(&after), 0, "{}", stderr(&after));
    assert_ne!(
        fixture(&stdout(&after), "model-calls"),
        "0",
        "它真的跑了：模型被调用"
    );
}

// ---------------------------------------------------------------------------
// 9. 行动明确失败之后：是结果，不是整个活动的失败
// ---------------------------------------------------------------------------

#[test]
fn an_explicitly_failed_action_is_a_result_the_subject_continues_from() {
    let temp = TempDir::new("me-action-failure");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    std::fs::write(materials.join("note.txt"), "资料正文").expect("写资料");
    let created = cli(&[
        "init",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-failure",
    ]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    // 展开（成功）→ 先看格式（成功）→ 读一个不存在的文件（**明确失败**）→ 主体自己决定收场。
    let run = Fixture::new(&root, "agent-failure", &materials, "run-failure", "script")
        .max_turns(4)
        .script("act:open_app?target=files;act:inspect_action?action=files_read;act:files_read?path=missing.txt;finish")
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    let report = stdout(&run);
    assert_eq!(
        field(&report, "ending"),
        "subject:ended",
        "一次行动失败不等于整个活动失败：失败是交回主体的结果，不是系统替它收场"
    );
    assert_eq!(
        fixture(&report, "model-calls"),
        "4",
        "四次调用分别对应展开、先看格式、读失败与主体收场：系统既不自动重试，也不替它决定"
    );

    // 用**既有诊断命令**交叉验证：这次活动的事实里恰好有一条明确失败与两条明确成功
    // （展开、先看格式）、三条准备，而且它仍然以主体收场结束——这正是"失败被记成结果"的可判错证据。
    let diagnosed = diagnose(&root, "agent-failure", "run-failure");
    let line = activity_line(&diagnosed, "run-failure");
    assert!(line.contains("failure=1"), "{line}");
    assert!(line.contains("success=2"), "{line}");
    assert!(line.contains("prepared=3"), "{line}");
    assert!(line.contains("outcome=subject:ended"), "{line}");

    // 诊断报的失败类别来自文件 App 的既有分类，不是入口编的。
    let facts = diagnosed
        .lines()
        .filter(|line| line.starts_with("fact "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(facts.contains("state=failure"), "{facts}");
}

// ---------------------------------------------------------------------------
// 夹具与工具
// ---------------------------------------------------------------------------

/// 创建授权参数：只在 `run` 上显式给出，旧参数与旧入口不因此获得写权限。
#[test]
fn the_creation_flag_is_explicit_and_never_implicit() {
    let temp = TempDir::new("me-create-flag");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    let root_text = path_text(&root);
    let materials_text = path_text(&materials);

    let created = cli(&["init", "--root", &root_text, "--agent", "agent-create"]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));
    let before = snapshot(&root);

    // init 不接受创建授权：那是运行期的事，给了就是写错了。
    let init_with_flag = cli(&[
        "init",
        "--root",
        &root_text,
        "--agent",
        "agent-create",
        "--create-dir",
        "notes",
    ]);
    assert_eq!(code(&init_with_flag), 1);
    assert_eq!(snapshot(&root), before, "被拒绝的 init 不得改动环境");

    // 授权目录必须是一个合法的单层目录名：这些都是入口拒绝，退出码 1。
    for bad in [".", "..", "a/b", "C:////x", "notes/../x"] {
        let rejected = cli(&[
            "run",
            "--root",
            &root_text,
            "--agent",
            "agent-create",
            "--materials",
            &materials_text,
            "--run",
            "run-1",
            "--max-turns",
            "2",
            "--create-dir",
            bad,
        ]);
        assert_eq!(code(&rejected), 1, "{bad} 必须被入口拒绝");
        assert!(
            stderr(&rejected).contains("--create-dir 不合法"),
            "{bad}: {}",
            stderr(&rejected)
        );
    }

    // 不给 --create-dir 就是默认只读：这正是旧入口的行为，一个字都没变。
    let read_only = cli(&[
        "run",
        "--root",
        &root_text,
        "--agent",
        "agent-create",
        "--materials",
        &materials_text,
        "--run",
        "run-1",
        "--max-turns",
        "2",
    ]);
    assert_eq!(
        code(&read_only),
        5,
        "普通 run 仍然只到装配模型这一步（没有凭据就停在这里）"
    );
    assert_eq!(snapshot(&root), before, "没有凭据的时刻不得改动环境");
}

// ---------------------------------------------------------------------------
// 13. **跨进程恢复已退役**（2026-09-26）：这里只剩"那条路真的不在了"这一条读数
// ---------------------------------------------------------------------------
//
// ⚠ **旧规矩是什么、新规矩是什么**。这一段原来有六条用例，证明的是"两个真实进程走同一条闭环"：
// 进程 A 停在等待 → 进程 B 用 `agent_run resume` **顶着同一个 RunId** 接着跑——
// 判定在先（10/11/13 三个码）、零模型调用直到显式宿主动作、请求从已确认记录重建（含协议续接
// 原文）、登记与输入不重写、预算不重置；还有一条是"带约束继续 + 未决效果门"（写类别在复核解除
// 之前被拒）。
//
// **那条路整条退役**：承载活动的进程没了，就是两个 Run——那一次连续
// 就断了；新进程起来是**新的一段**，不是"顶着同一个 RunId 继续"。于是 `agent_run resume`、
// `ManualResume`、`RunSession::resume`、`Run::resume`、恢复的退出码 10/11/13，以及**写类别门**
// （它唯一的装门路径就是"带约束继续"）一起删除。
//
// **逐条去向**（为什么不留、也不搬）：
//
// - `two_processes_recover_the_same_run_and_it_ends_where_it_started`：**随路退役**。
//   "请求从记录重建"没有载体了（水化入口删了）；"同一个 RunId 只登记一次"由既有准入与
//   `store` 自己守着（`tests/durable_environment.rs`）。
// - `two_processes_resume_a_dispatched_write_with_the_pending_effect_gate_armed`：**随路退役**
//   （门退役了，见 `tests/pending_effect_gate.rs` 的删除）。
// - `a_recovered_process_rebuilds_the_paired_request_and_the_thinking_continuation`：**随路退役**
//   ——"协议续接原文必须被带回"这一条**不变量还在**，但它的载体是**渲染**，已由
//   `tests/request_recomputability.rs` 用"同一份 View 两次请求逐字节相同"接住。
// - `a_resume_without_an_explicit_host_action_makes_no_request`：**随路退役**（"恢复 ≠ 自动继续"
//   这句话没有主语了：新的一段由它自己的开场驱动）。
// - `the_resume_decision_comes_before_a_credential_is_needed`：**随路退役**。它守的那条顺序
//   （"环境层面的拒绝排在装配模型之前"）**在 `run` 那条路上照旧**，由本文件第 12 节
//   `the_creation_flag_is_explicit_and_never_implicit` 与 `refusals_never_reach_the_model_and_never_fabricate_facts`
//   守着。
// - `the_resume_subcommand_rejects_run_only_options`：**换载体**——见下面那条用例：那条路现在
//   是一条**未知子命令**。

/// `resume` 现在是**未知子命令**：入口明确拒绝，不静默忽略、也不留兼容分支。
///
/// ⚠ **旧规矩是什么、新规矩是什么**：这里原来断言的是"`resume` 不接受 `--input` /
/// `--input-stdin` / `--host-input`"（因为输入在盘上）。**新规矩**：整条路退役，于是这个子命令
/// 连"存在但挑参数"都不是了——它必须像任何写错的子命令一样，在**打开环境、装配模型之前**
/// 就被拒（退出码 1，一个字节都不写）。
#[test]
fn the_resume_subcommand_is_now_an_unknown_subcommand() {
    let temp = TempDir::new("me-resume-retired");
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");

    let root_text = path_text(&root);
    let materials_text = path_text(&materials);
    let output = cli(&[
        "resume",
        "--root",
        &root_text,
        "--agent",
        "agent-a",
        "--materials",
        &materials_text,
        "--run",
        "run-1",
        "--max-turns",
        "2",
    ]);
    assert_eq!(
        code(&output),
        1,
        "未知子命令是入口拒绝: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("未知子命令: resume"),
        "必须说清它是一条不存在的子命令（而不是「参数写错了」）: {}",
        stderr(&output)
    );
    assert!(
        !root.exists(),
        "入口拒绝必须排在任何环境动作之前：一个字节都不该写"
    );
}

/// 生产命令：凭据一律从子进程环境里摘掉，避免任何真实 Key 进入测试。
fn cli(arguments: &[&str]) -> Output {
    Command::new(binary("agent_run"))
        .args(arguments)
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .expect("启动生产命令")
}

fn cli_with_stdin(arguments: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(binary("agent_run"))
        .args(arguments)
        .env_remove("DEEPSEEK_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动生产命令");
    child
        .stdin
        .as_mut()
        .expect("有标准输入")
        .write_all(input)
        .expect("写标准输入");
    child.wait_with_output().expect("等待命令结束")
}

/// 离线夹具：一次运行的全部参数。
struct Fixture {
    variables: Vec<(&'static str, String)>,
}

impl Fixture {
    fn new(root: &Path, agent: &str, materials: &Path, run: &str, model: &str) -> Self {
        Self {
            variables: vec![
                ("MANUAL_FIXTURE_ROOT", path_text(root)),
                ("MANUAL_FIXTURE_AGENT", agent.to_string()),
                ("MANUAL_FIXTURE_MATERIALS", path_text(materials)),
                ("MANUAL_FIXTURE_RUN", run.to_string()),
                ("MANUAL_FIXTURE_MAX_TURNS", "2".to_string()),
                ("MANUAL_FIXTURE_MODEL", model.to_string()),
            ],
        }
    }

    fn max_turns(mut self, turns: u32) -> Self {
        self.set("MANUAL_FIXTURE_MAX_TURNS", turns.to_string());
        self
    }

    fn script(mut self, script: &str) -> Self {
        self.set("MANUAL_FIXTURE_SCRIPT", script.to_string());
        self
    }

    fn input(mut self, text: &str) -> Self {
        self.set("MANUAL_FIXTURE_INPUT", text.to_string());
        self
    }

    fn probe(mut self, text: &str) -> Self {
        self.set("MANUAL_FIXTURE_PROBE", text.to_string());
        self
    }

    /// 显式启用**宿主输入循环**，并按 `\n` 分隔给出后续几行宿主动作（空值 = 立刻 EOF）。
    fn host_lines(mut self, lines: &str) -> Self {
        self.set("MANUAL_FIXTURE_HOST_LINES", lines.to_string());
        self
    }

    fn endpoint(mut self, endpoint: &str, credential: &str) -> Self {
        self.set("MANUAL_FIXTURE_ENDPOINT", endpoint.to_string());
        self.set("MANUAL_FIXTURE_CREDENTIAL", credential.to_string());
        self.set("MANUAL_FIXTURE_MODEL_NAME", "fixture-model".to_string());
        self
    }

    fn set(&mut self, name: &'static str, value: String) {
        if let Some(entry) = self.variables.iter_mut().find(|entry| entry.0 == name) {
            entry.1 = value;
        } else {
            self.variables.push((name, value));
        }
    }

    fn run(self) -> Output {
        let mut command = Command::new(binary("manual_entry_fixture"));
        for (name, value) in &self.variables {
            command.env(name, value);
        }
        // 显式禁代理：回环地址不该被任何代理环境变量劫持；同时把真实凭据挡在外面。
        command
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .env_remove("DEEPSEEK_API_KEY")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("all_proxy");
        command.output().expect("启动夹具")
    }
}

fn spawn_worker(role: &str, root: &Path) -> Output {
    Command::new(binary("environment_worker"))
        .env("CELIRA_PHASE08_ROLE", role)
        .env("CELIRA_PHASE08_ROOT", root)
        .output()
        .expect("启动环境夹具")
}

/// 回环服务：第一轮答一次行动，第二轮答收场；返回端点与两轮请求正文。
fn spawn_reply_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        // 两轮应答都按**原生工具调用**的形状：`tool_calls[].function` + 字符串里的 JSON 参数。
        const ACTION: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-expand","type":"function","function":{"name":"open_app","arguments":"{\"target\":\"files\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        const FINISH: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish_run","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let mut bodies = Vec::new();
        for reply in [ACTION, FINISH] {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            bodies.push(read_request_body(&mut stream));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
        }
        let _ = sender.send(bodies.join("\n=====REQUEST=====\n"));
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        receiver,
    )
}

/// 按脚本应答的回环服务：返回端点与全部请求正文（用分隔符连起来）。
///
/// 脚本用完之后**不再盯**多出来的连接：这里的证明点是"第几轮请求里有什么"，零自动重试由
/// `spawn_reply_server` 那条（带观察窗口）与 `manual_entry` 的拒绝路径各自负责。
fn spawn_scripted_server(replies: Vec<&'static str>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            bodies.push(read_request_body(&mut stream));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
        }
        let _ = sender.send(bodies.join("\n=====REQUEST=====\n"));
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        receiver,
    )
}

/// 回环服务：按脚本应答，并把**请求原文**（请求行 + 头 + 体）交给用例。
///
/// 与 [`spawn_scripted_server`] 的区别只在捕获面：那条只留请求体，这条连头一起留——
/// 凭据只进头，因此"凭据头来自配对的那个变量"这条读数必须看原文。
fn spawn_raw_scripted_server(replies: Vec<&'static str>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let mut raws = Vec::new();
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            raws.push(read_raw_request(&mut stream));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
        }
        let _ = sender.send(raws.join("\n=====REQUEST=====\n"));
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        receiver,
    )
}

/// 读一段**请求原文**：请求行与全部头，加上按 `Content-Length` 读到的体。
fn read_raw_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::{BufRead, BufReader, Read};
    let mut reader = BufReader::new(stream.try_clone().expect("可以克隆流"));
    let mut raw = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim_end().to_string();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        raw.push_str(&trimmed);
        raw.push('\n');
    }
    if length > 0 {
        let mut body = vec![0u8; length];
        let mut filled = 0usize;
        while filled < length {
            match reader.read(&mut body[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(_) => break,
            }
        }
        raw.push_str(&String::from_utf8_lossy(&body));
    }
    raw
}

/// 一个只监听、不应答的端点：用来证明拒绝路径上没有请求出网。
fn spawn_silent_endpoint() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        listener,
    )
}

/// 在给定时限内没有连接才算通过；一旦有连接就判失败。
fn assert_no_request(listener: &TcpListener) {
    listener.set_nonblocking(true).expect("设成非阻塞");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok(_) => panic!("拒绝路径不得发出任何请求"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

fn read_request_body(stream: &mut std::net::TcpStream) -> String {
    use std::io::{BufRead, BufReader, Read};
    let mut reader = BufReader::new(stream.try_clone().expect("可以克隆流"));
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.to_ascii_lowercase().starts_with("content-length:") {
            length = line
                .split(':')
                .nth(1)
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
        }
        if line == "\r\n" {
            break;
        }
    }
    let mut buffer = vec![0u8; length];
    if reader.read_exact(&mut buffer).is_ok() {
        String::from_utf8_lossy(&buffer).to_string()
    } else {
        String::new()
    }
}

/// 既有诊断命令：交叉验证入口之后，事实里到底记下了什么。
fn diagnose(root: &Path, agent: &str, run: &str) -> String {
    let output = Command::new(binary("environment_diagnose"))
        .args([
            path_text(root),
            agent.to_string(),
            "--run".to_string(),
            run.to_string(),
        ])
        .output()
        .expect("启动诊断命令");
    assert_eq!(
        output.status.code(),
        Some(0),
        "诊断应当成功: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// 诊断报告里某个活动的那一行。
fn activity_line(text: &str, run: &str) -> String {
    let needle = format!("run={run} ");
    text.lines()
        .find(|line| line.starts_with("activity ") && line.contains(&needle))
        .unwrap_or_else(|| panic!("诊断报告里没有 {run} 的活动行: {text}"))
        .to_string()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// 报告里的 `键=值`（空格分隔的字段）。
fn field(text: &str, key: &str) -> String {
    let needle = format!("{key}=");
    for line in text.lines() {
        for token in line.split(' ') {
            if let Some(value) = token.strip_prefix(&needle) {
                return value.to_string();
            }
        }
    }
    panic!("报告里没有字段 {key}: {text}");
}

/// 夹具自己的 `FIXTURE:键=值` 行。
fn fixture(text: &str, key: &str) -> String {
    let needle = format!("FIXTURE:{key}=");
    text.lines()
        .find_map(|line| line.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("夹具报告里没有 {key}: {text}"))
        .to_string()
}

/// 目录快照：全部普通文件的名字与字节，按名字排序。
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("读目录")
        .map(|entry| entry.expect("目录项可读"))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(entry.path()).unwrap_or_default();
            (name, bytes)
        })
        .collect();
    entries.sort();
    entries
}

/// 输入的三种来源各自有稳定标记（纯查表，不需要模型）。
#[test]
fn input_sources_have_stable_labels() {
    assert_eq!(ManualInput::Empty.as_str(), "empty");
    assert_eq!(ManualInput::Text("x".into()).as_str(), "text");
    assert_eq!(ManualInput::Stdin.as_str(), "stdin");
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("celira-task04-{label}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// **档位门控（生产命令）**：`--terminal` 收一个**封闭枚举**的档位。
///
/// 两条读数是这一格的承重部分：
/// - **不认识的档位** ⇒ 退出码 1（参数错，与别的参数错同族），而且**环境都没被建**——
///   拒绝发生在打开环境之前；
/// - 认识的档位（`full`）**过得去这一关**（卡在"环境不存在 ⇒ 2"），
///   证明上一条拒绝的是**档位**，不是"这个选项本身"。
#[test]
fn the_terminal_tier_is_a_closed_enum_in_the_production_command() {
    let temp = TempDir::new("terminal-tier");
    let root = temp.path().join("env-never-created");
    let materials = temp.path().join("materials");

    let unknown = cli(&[
        "run",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-tier",
        "--max-turns",
        "2",
        "--terminal",
        "home-write",
    ]);
    assert_eq!(code(&unknown), 1, "未知档位是参数错\n{}", stderr(&unknown));
    assert!(
        stderr(&unknown).contains("未知的 terminal 档位"),
        "要点名是档位不认识，并且**不静默降档**：{}",
        stderr(&unknown)
    );
    assert!(
        !root.join("facts.ndjson").exists(),
        "拒绝发生在打开环境之前：环境不该被建出来"
    );

    let known = cli(&[
        "run",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-tier",
        "--max-turns",
        "2",
        "--terminal",
        "full",
    ]);
    assert_eq!(
        code(&known),
        2,
        "认得的档位过关，下一步才是「环境不存在」\n{}",
        stderr(&known)
    );
}

/// **出网档（生产命令）**：`--web` 与 `--terminal` 同一形状——**封闭枚举**，不认识的档位不降级。
///
/// 两条读数是这一格的承重部分：
/// - **不认识的档位** ⇒ 退出码 1（参数错），而且**环境都没被建**——拒绝发生在打开环境之前；
/// - 认识的档位（`fetch`）**过得去这一关**（卡在"环境不存在 ⇒ 2"），
///   证明上一条拒绝的是**档位**，不是"这个选项本身"。
///
/// ⚠ 判据是**"她能不能出网"是主人的政策**（与"她能不能拿到 shell"同一类事实）：猜错就是替主人做主。
#[test]
fn the_web_tier_is_a_closed_enum_in_the_production_command() {
    let temp = TempDir::new("web-tier");
    let root = temp.path().join("env-never-created");
    let materials = temp.path().join("materials");

    let unknown = cli(&[
        "run",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-web-tier",
        "--max-turns",
        "2",
        "--web",
        "sometimes",
    ]);
    assert_eq!(code(&unknown), 1, "未知档位是参数错\n{}", stderr(&unknown));
    assert!(
        stderr(&unknown).contains("未知的 web 档位"),
        "要点名是档位不认识，并且**不静默降档**：{}",
        stderr(&unknown)
    );
    assert!(
        !root.join("facts.ndjson").exists(),
        "拒绝发生在打开环境之前：环境不该被建出来"
    );

    let known = cli(&[
        "run",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-a",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-web-tier",
        "--max-turns",
        "2",
        "--web",
        "fetch",
    ]);
    assert_eq!(
        code(&known),
        2,
        "认得的档位过关，下一步才是「环境不存在」\n{}",
        stderr(&known)
    );
}

// ---------------------------------------------------------------------------
// TASK_38：模型源参数化（两档）
// ---------------------------------------------------------------------------

/// 一次收场（原生工具调用形状）：换家档的"真发"读数只需要一轮。
const MODEL_FINISH: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish_run","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

/// 带额外环境变量的生产命令（凭据一律从子进程环境里给：**只进环境，不进 argv**）。
fn cli_env(arguments: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut command = Command::new(binary("agent_run"));
    command.args(arguments).env_remove("DEEPSEEK_API_KEY");
    for (name, value) in extra {
        command.env(name, value);
    }
    command.output().expect("启动生产命令")
}

/// 建一个可用的环境（换家档的读数要真跑到模型那一层，因此环境必须存在）。
fn initialized(temp: &TempDir, label: &str) -> (PathBuf, PathBuf) {
    let root = temp.path().join("env");
    let materials = temp.path().join("materials");
    std::fs::create_dir_all(&materials).expect("建资料目录");
    let created = cli(&["init", "--root", &path_text(&root), "--agent", label]);
    assert_eq!(code(&created), 0, "{}", stderr(&created));
    (root, materials)
}

/// **换家档四件套真发**：端点 / 凭据 / 模型名 / 标称窗口四件齐了 ⇒ 请求**真的到了那个端点**，
/// 而且请求体里的模型名与凭据头都来自这一次声明；凭据值不进任何输出。
#[test]
fn the_custom_endpoint_four_pieces_reach_the_wire() {
    const SENTINEL: &str = "sentinel-model-credential";
    let temp = TempDir::new("model-endpoint");
    let (root, materials) = initialized(&temp, "agent-model");
    let (endpoint, bodies) = spawn_raw_scripted_server(vec![MODEL_FINISH]);

    let run = cli_env(
        &[
            "run",
            "--root",
            &path_text(&root),
            "--agent",
            "agent-model",
            "--materials",
            &path_text(&materials),
            "--run",
            "run-model",
            "--max-turns",
            "1",
            "--input",
            "醒着",
            "--model-endpoint",
            &endpoint,
            "--model",
            "glm-4.6",
            "--context-window",
            "200000",
        ],
        &[("CELIRA_MODEL_API_KEY", SENTINEL)],
    );
    let report = stdout(&run);
    assert_eq!(
        code(&run),
        0,
        "四件齐了就该真跑起来：stdout={report} stderr={}",
        stderr(&run)
    );
    assert_eq!(field(&report, "ending"), "subject:ended");

    let request = bodies
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("请求应当打到那个自定义端点");
    assert!(
        request.contains("\"model\":\"glm-4.6\""),
        "模型名来自宿主声明，不是默认值：{request}"
    );
    assert!(
        request.contains(&format!("Bearer {SENTINEL}")),
        "凭据头来自配对的那个变量：{request}"
    );
    // 窗口是**声明**：它进状态行（模型看得见的那份处境），因此请求体里读得到它——
    // 而且读到的必须是宿主声明的那一个（不是默认档的标称值）。
    assert!(
        request.contains("/200000"),
        "标称窗口用宿主声明的那一个（默认档的标称值是别的数）：{request}"
    );
    // 凭据值零泄漏：命令自己的输出里一个字节都不许出现。
    assert!(
        !report.contains(SENTINEL) && !stderr(&run).contains(SENTINEL),
        "凭据值不许进任何输出"
    );
}

/// **成组拒绝，一件一件来**：给了端点之后，另外三件缺哪件就报哪件（退出码 5），
/// 而且**一个请求都不发**（拒绝发生在出网之前）。
#[test]
fn the_four_pieces_are_refused_one_by_one_without_any_request() {
    let temp = TempDir::new("model-refusals");
    let (root, materials) = initialized(&temp, "agent-model");
    let (endpoint, listener) = spawn_silent_endpoint();
    let base = [
        "run",
        "--root",
        &path_text(&root),
        "--agent",
        "agent-model",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-refuse",
        "--max-turns",
        "1",
        "--input",
        "醒着",
        "--model-endpoint",
        &endpoint,
    ];

    // (1) 缺凭据
    let mut missing_credential: Vec<&str> = base.to_vec();
    missing_credential.extend(["--model", "glm-4.6", "--context-window", "200000"]);
    let refused = cli_env(&missing_credential, &[]);
    assert_eq!(code(&refused), 5, "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("CELIRA_MODEL_API_KEY"),
        "要点名缺的是哪个变量（只报名字、不报值）：{}",
        stderr(&refused)
    );

    // (2) 缺模型名
    let mut missing_model: Vec<&str> = base.to_vec();
    missing_model.extend(["--context-window", "200000"]);
    let refused = cli_env(&missing_model, &[("CELIRA_MODEL_API_KEY", "sentinel")]);
    assert_eq!(code(&refused), 5, "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("--model"),
        "要点名缺的是哪个旗标、以及为什么不猜：{}",
        stderr(&refused)
    );

    // (3) 缺窗口
    let mut missing_window: Vec<&str> = base.to_vec();
    missing_window.extend(["--model", "glm-4.6"]);
    let refused = cli_env(&missing_window, &[("CELIRA_MODEL_API_KEY", "sentinel")]);
    assert_eq!(code(&refused), 5, "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("--context-window"),
        "窗口必须重声明，且要点名是它缺了：{}",
        stderr(&refused)
    );

    // 三次拒绝都不许有请求出网（那个端点只监听、不应答）。
    assert_no_request(&listener);
}

/// **环境音**：`CELIRA_MODEL_API_KEY` 单独在场、没给端点 ⇒ 它不该改变分档：
/// 走默认档（凭据看 `DEEPSEEK_API_KEY`），失败消息也说的是那个变量。
#[test]
fn a_stray_custom_credential_does_not_change_the_tier() {
    let temp = TempDir::new("model-noise");
    let (root, materials) = initialized(&temp, "agent-model");

    let run = cli_env(
        &[
            "run",
            "--root",
            &path_text(&root),
            "--agent",
            "agent-model",
            "--materials",
            &path_text(&materials),
            "--run",
            "run-noise",
            "--max-turns",
            "1",
            "--input",
            "醒着",
        ],
        // 只给这一支：它是环境音，不是分档信号。
        &[("CELIRA_MODEL_API_KEY", "sentinel-not-used")],
    );
    assert_eq!(code(&run), 5, "{}", stderr(&run));
    assert!(
        stderr(&run).contains("DEEPSEEK_API_KEY"),
        "没给端点就是默认档 ⇒ 缺的凭据是默认档那一个：{}",
        stderr(&run)
    );
    assert!(
        !stderr(&run).contains("sentinel-not-used"),
        "环境音里的取值一个字节都不许进输出"
    );
}
