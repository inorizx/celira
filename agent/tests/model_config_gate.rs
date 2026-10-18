//! **模型配置门**（TASK_77）：环境根下那个私有文件与进程侧声明的关系。
//!
//! 这一件最要紧的四条判据都在这里钉住：
//!  1. **文件不在场 ⇒ 现状分毫不动**（听进程侧，横幅一行都不多）；
//!  2. **文件在场 ⇒ 以它为准**，四件（+ 可选协议）原样出来；
//!  3. **两边同时说话 ⇒ 拒绝**，而且错误里必须**说清是哪两个**（不猜、不静默覆盖）；
//!  4. **文件坏了 / 形状不对 ⇒ 如实失败**，绝不静默回落进程侧——
//!     回落会让"主体手里那支密钥"从台面下换掉，而操作者以为一切照旧。
//!
//! 还有一条**读数**判据：横幅是**洗凭据**的——它报来源名与键名，**永不报取值**。
//!
//! 刀2b 起多了**第三个状态**：这个 Agent **绑定了清单里的一条**
//! （`<环境根>/model_binding.json`）。绑定在场 ⇒ 四件从**清单**那一条解析出来，进程侧与老
//! 的单份配置都必须沉默；绑的那条被删了（悬空绑定）如实拒绝，**绝不回落** env 那一路。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::model_source::{self, FileSource, FlagDeclarations, Source};

/// 一次性的环境根（照本仓既有测试的临时目录约定）。
///
/// ⚠ 名字里**必须带进程号**：测试二进制是一份一个进程，而不带进程号的名字在两份并行
/// （或两个写者同时跑）时算出同一条路径、**互相删目录**（2026-10-09 实测）。
fn scratch(label: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("celira-model-gate-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建立临时环境根");
    root
}

/// 写一份**合法**的配置（四件 + 可选协议）。
fn write_config(root: &Path, endpoint: &str, credential: &str, model: &str, window: &str) {
    let text = format!(
        "{{\n  \"endpoint\": \"{endpoint}\",\n  \"api_key\": \"{credential}\",\n  \
         \"model\": \"{model}\",\n  \"context_window\": {window}\n}}\n"
    );
    fs::write(model_source::file_path(root), text).expect("写配置文件");
}

/// 没有任何进程侧声明的旗标。
fn no_flags() -> FlagDeclarations<'static> {
    FlagDeclarations {
        endpoint: None,
        model: None,
        context_window: None,
        protocol: None,
    }
}

/// 判据 1：文件不在场 ⇒ `Source::Process`，且横幅**一行都不多**（现状零改动）。
#[test]
fn a_missing_file_leaves_the_process_side_alone() {
    let root = scratch("missing");
    let source = model_source::decide(&root, None, &no_flags(), &[]).expect("文件不在场不该是错误");
    assert!(matches!(source, Source::Process));
    assert_eq!(source.origin(), "process");
    assert!(
        model_source::banner_lines(&source).is_empty(),
        "以进程侧为源时横幅不该多出任何一行"
    );
}

/// 判据 2：文件在场 ⇒ 以它为准，四件原样出来；协议缺省（`None` = 本家那一条）。
#[test]
fn a_present_file_is_the_source_and_carries_the_four_declared_pieces() {
    let root = scratch("present");
    write_config(
        &root,
        "https://example.invalid/v1/chat",
        "sk-value",
        "some-model",
        "131072",
    );

    let source = model_source::decide(&root, None, &no_flags(), &[]).expect("读得成");
    let Source::File(file) = &source else {
        panic!("文件在场就该以文件为源");
    };
    let FileSource {
        endpoint,
        credential,
        model,
        context_window,
        protocol,
        ..
    } = &**file;
    assert_eq!(endpoint, "https://example.invalid/v1/chat");
    assert_eq!(credential, "sk-value");
    assert_eq!(model, "some-model");
    assert_eq!(*context_window, 131072);
    assert_eq!(*protocol, None, "缺省的那一件不该被凭空补上");
}

/// 判据 2b（**读数**）：横幅报来源名 / 端点 / 模型名 / 键名 —— **永不报取值**。
#[test]
fn the_banner_reports_the_source_and_never_the_credential_value() {
    let root = scratch("banner");
    write_config(
        &root,
        "https://example.invalid/v1/chat",
        "sk-must-not-leak",
        "some-model",
        "1024",
    );
    let source = model_source::decide(&root, None, &no_flags(), &[]).expect("读得成");

    let lines = model_source::banner_lines(&source).join("\n");
    assert!(lines.contains("model-source=file"), "{lines}");
    assert!(lines.contains("https://example.invalid/v1/chat"), "{lines}");
    assert!(lines.contains("some-model"), "{lines}");
    assert!(lines.contains("model-credential=api_key"), "{lines}");
    // **哪一条 wire 真的在跑**（2026-10-11 补）：这一格永远有值（缺省 = 本家那一条），
    // 所以"没声明"也必须报一行——否则操作者只能对着清单猜"我声明的生效了没有"。
    assert!(
        lines.contains("model-protocol="),
        "横幅要报生效的线协议: {lines}"
    );
    assert!(
        !lines.contains("sk-must-not-leak"),
        "横幅是洗凭据的：它只报键名，取值一个字节都不许出现"
    );
}

/// 判据 3（旗标那一边）：文件在场 + 任何模型旗标 ⇒ 拒绝，错误里两个来源都在。
#[test]
fn a_file_plus_a_command_line_declaration_is_refused_with_both_names() {
    let root = scratch("conflict-flag");
    write_config(
        &root,
        "https://example.invalid/v1/chat",
        "sk-value",
        "some-model",
        "1024",
    );
    let error = model_source::decide(
        &root,
        None,
        &FlagDeclarations {
            endpoint: Some("https://other.invalid/v1/chat"),
            ..no_flags()
        },
        &[],
    )
    .expect_err("两边同时说话就该拒绝");
    assert!(error.contains("model_source.json"), "{error}");
    assert!(error.contains("--model-endpoint"), "{error}");
}

/// 判据 3（凭据环境变量那一边）：**2026-10-08 主人裁决 —— 它不再算"那一边在说话"**。
///
/// 裁决原话："**配置模型是主体自己的事情**"。凭据环境变量与绑定声明的四件套**不重叠**
/// （清单里那一条自带钥匙），而且绑定在场时它**永不被读** ⇒ 让它否决主体自己的配置，就是把
/// 开发期的兜底当成了配置的一档。⇒ 现在的判据：**以文件为源、照常装配**，
/// 并把那条变量的**名字**记进 `superseded_credential`（由横幅如实报一行"它没被用"）。
///
/// ⚠️ 这里用 `PATH` 当那条凭据变量的名字，是因为**本仓测试不碰进程环境**（不 `set_var`）：
/// `PATH` 在每个进程里都必然非空，正好当"那条变量非空"的替身。
#[test]
fn a_credential_variable_no_longer_vetoes_the_agents_own_configuration() {
    let root = scratch("superseded-env");
    write_config(
        &root,
        "https://example.invalid/v1/chat",
        "sk-value",
        "some-model",
        "1024",
    );
    let source = model_source::decide(&root, None, &no_flags(), &["PATH"])
        .expect("凭据环境变量不该再否决主体自己的配置");
    let Source::File(file) = &source else {
        panic!("文件在场就该以文件为源");
    };
    assert_eq!(
        file.superseded_credential.as_deref(),
        Some("PATH"),
        "被取代的那条要记下来（横幅据此报一行），而且只记名字"
    );
    let banner = model_source::banner_lines(&source).join("\n");
    assert!(
        banner.contains("PATH"),
        "横幅要如实报出它被取代了：{banner}"
    );
    assert!(!banner.contains("sk-value"), "横幅永不报取值：{banner}");

    // 负向对照：一条**不存在**的名字 ⇒ 没有遗留可报。
    let clean = model_source::decide(
        &root,
        None,
        &no_flags(),
        &["CELIRA_NO_SUCH_CREDENTIAL_MARKER"],
    )
    .expect("本来就不该拒绝");
    let Source::File(file) = &clean else {
        panic!("文件在场就该以文件为源");
    };
    assert_eq!(file.superseded_credential, None);
}

/// 判据 4：文件坏了 / 形状不对 ⇒ **如实失败**，而且**不是**回落进程侧。
#[test]
fn a_broken_file_fails_honestly_instead_of_falling_back() {
    let root = scratch("broken");
    fs::write(model_source::file_path(&root), "{ 这不是 JSON").expect("写坏文件");
    let error = model_source::decide(&root, None, &no_flags(), &[]).expect_err("读不出来就该失败");
    assert!(error.contains("JSON"), "{error}");
    // 负向对照：错误里不能出现"改用进程侧"这种兜底语义（那正是本刀要堵的静默回落）。
    assert!(!error.contains("改用"), "{error}");
}

/// 判据 4b：四件缺一 / 窗口不是正整数 ⇒ 点名**是哪一件**。
#[test]
fn each_missing_piece_is_named_in_the_refusal() {
    let root = scratch("missing-piece");
    for (label, text, key) in [
        (
            "缺端点",
            r#"{"api_key":"k","model":"m","context_window":1024}"#,
            "endpoint",
        ),
        (
            "缺密钥",
            r#"{"endpoint":"https://example.invalid","model":"m","context_window":1024}"#,
            "api_key",
        ),
        (
            "缺模型名",
            r#"{"endpoint":"https://example.invalid","api_key":"k","context_window":1024}"#,
            "model",
        ),
        (
            "空密钥",
            r#"{"endpoint":"https://example.invalid","api_key":"   ","model":"m","context_window":1024}"#,
            "api_key",
        ),
        (
            "窗口是 0",
            r#"{"endpoint":"https://example.invalid","api_key":"k","model":"m","context_window":0}"#,
            "context_window",
        ),
        (
            "窗口是字符串",
            r#"{"endpoint":"https://example.invalid","api_key":"k","model":"m","context_window":"big"}"#,
            "context_window",
        ),
    ] {
        fs::write(model_source::file_path(&root), text).expect("写配置文件");
        let error = model_source::decide(&root, None, &no_flags(), &[]).unwrap_err();
        assert!(
            error.contains(key),
            "{label}：错误里该点名 {key} —— {error}"
        );
    }
}

/// 判据 2c：可选协议给了就原样交出来（**闭集校验在装配入口那一处**，不在这里造第二份）。
#[test]
fn the_optional_protocol_is_handed_through_verbatim() {
    let root = scratch("protocol");
    fs::write(
        model_source::file_path(&root),
        r#"{"endpoint":"https://example.invalid","api_key":"k","model":"m",
            "context_window":2048,"protocol":"responses"}"#,
    )
    .expect("写配置文件");
    let source = model_source::decide(&root, None, &no_flags(), &[]).expect("读得成");
    let Source::File(file) = source else {
        panic!("文件在场就该以文件为源");
    };
    assert_eq!(file.protocol.as_deref(), Some("responses"));
    assert_eq!(file.context_window, 2048);
}

// ---------------------------------------------------------------------------
// 刀2b：**绑定 → 清单**（清单归机器，绑定归 Agent）
// ---------------------------------------------------------------------------

/// 机器级的清单住在**家**里，不在环境根里——那是"房子那一份"。
fn write_one_entry_list(
    home: &Path,
    endpoint: &str,
    credential: &str,
    model: &str,
    window: u32,
) -> PathBuf {
    let file = home.join("model_sources.json");
    let text = format!(
        "[\n  {{\n    \"id\": \"ms-1\",\n    \"name\": \"默认\",\n    \"endpoint\": \"{endpoint}\",\n    \
         \"api_key\": \"{credential}\",\n    \"model\": \"{model}\",\n    \
         \"context_window\": {window}\n  }}\n]\n"
    );
    fs::write(&file, text).expect("写清单");
    file
}

/// 写绑定：这个 Agent 用清单里的哪一条。
fn write_binding(root: &Path, source: &str) {
    let text = format!("{{ \"source\": \"{source}\" }}\n");
    fs::write(model_source::binding_path(root), text).expect("写绑定");
}

/// 判据 5：绑定了但没有声明清单在哪 ⇒ **拒绝**，话里给出补法（`--model-sources`）。
///
/// 不回落进程侧：绑定说的是"这一个 Agent 用哪一条"，解析不出来是配置缺一条路，不是"改用别的"。
#[test]
fn a_binding_without_a_list_path_is_refused_with_a_way_out() {
    let root = scratch("binding-no-list");
    write_binding(&root, "ms-1");
    let error =
        model_source::decide(&root, None, &no_flags(), &[]).expect_err("解析不出来就该拒绝");
    assert!(error.contains("ms-1"), "{error}");
    assert!(error.contains("--model-sources"), "{error}");
}

/// 判据 6：绑定 + 清单 ⇒ 四件从**清单那一条**解析出来。
///
/// 两格读数一起钉住"从哪一份的哪一条来的"：`entry_id` 是清单里的 id，`path` 是**清单文件**的
/// 路径（不是老形状那个四件套文件的路径）。
#[test]
fn a_binding_resolves_the_four_pieces_from_the_list() {
    let home = scratch("binding-home");
    let root = scratch("binding-env");
    let list = write_one_entry_list(
        &home,
        "https://example.invalid/v1/chat",
        "sk-from-list",
        "listed-model",
        65536,
    );
    write_binding(&root, "ms-1");

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[]).expect("读得成");
    let Source::File(file) = &source else {
        panic!("绑定了清单里的一条就该以文件为源");
    };
    let FileSource {
        path,
        entry_id,
        endpoint,
        credential,
        model,
        context_window,
        protocol,
        reasoning_effort,
        vision,
        superseded_credential,
    } = &**file;
    assert_eq!(entry_id.as_deref(), Some("ms-1"), "要说清是从哪一条来的");
    assert_eq!(path, &list, "读数指的该是清单文件");
    assert_eq!(endpoint, "https://example.invalid/v1/chat");
    assert_eq!(credential, "sk-from-list");
    assert_eq!(model, "listed-model");
    assert_eq!(*context_window, 65536);
    assert_eq!(*protocol, None, "缺省的那一件不该被凭空补上");
    assert_eq!(*superseded_credential, None, "进程侧没有遗留 ⇒ 那一格空着");
    assert_eq!(
        *reasoning_effort, None,
        "没声明思考强度 ⇒ 那个键根本不发（默认档请求体逐字节不变）"
    );
    assert!(
        !*vision,
        "没声明视觉 ⇒ **不支持**（fail-closed）：工具面因此不摆出「她读得动图片」这句话"
    );
    assert_eq!(source.origin(), "file");
}

/// 判据 7：绑定与**老的单份配置**同时在场 ⇒ **双权威**，拒绝，而且两个文件都点出来。
#[test]
fn a_binding_plus_the_legacy_file_is_refused_as_two_authorities() {
    let home = scratch("double-home");
    let root = scratch("double-env");
    let list = write_one_entry_list(&home, "https://example.invalid", "sk", "m", 1024);
    write_binding(&root, "ms-1");
    write_config(
        &root,
        "https://legacy.invalid",
        "sk-legacy",
        "legacy-model",
        "2048",
    );

    let error = model_source::decide(&root, Some(&list), &no_flags(), &[])
        .expect_err("两个权威同时在场就该拒绝");
    assert!(error.contains("model_binding.json"), "{error}");
    assert!(error.contains("model_source.json"), "{error}");
    assert!(error.contains("ms-1"), "该说清绑到了哪一条 —— {error}");
}

/// 判据 8（负向对照组 / 最容易漏的那一格）：**留档件不是权威**。
///
/// 迁移把老文件改名成 `<名>.migrated`；它还在同一个目录里。若判据宽松成"目录里有像老文件的
/// 东西就算在场"，迁完之后主体会因为一个留档件**再也起不来**（永远双权威）。
#[test]
fn the_migrated_archive_is_not_an_authority() {
    let home = scratch("archive-home");
    let root = scratch("archive-env");
    let list = write_one_entry_list(&home, "https://example.invalid", "sk", "m", 1024);
    write_binding(&root, "ms-1");
    // 留档件：同一个目录、名字以老文件名**开头**。
    fs::write(root.join("model_source.json.migrated"), "{ 随便什么内容 }").expect("写留档件");

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[])
        .expect("留档件不是权威，不该让这次装配失败");
    assert!(matches!(source, Source::File(_)));

    // 另一半：连绑定也没有时，留档件同样不该被当成"老形状在场"（那会让主体去读一个留档件）。
    let bare = scratch("archive-bare");
    fs::write(bare.join("model_source.json.migrated"), "{ 随便什么内容 }").expect("写留档件");
    let source = model_source::decide(&bare, None, &no_flags(), &[]).expect("没有来源就是进程侧");
    assert!(matches!(source, Source::Process));
}

/// 判据 9：**悬空绑定**（绑的那条被删了）⇒ 拒绝，且明说"已被删除"、给出出路；
/// 绝不回落进程侧（回落会让主体拿另一支密钥上桌，而操作者以为一切照旧）。
#[test]
fn a_dangling_binding_is_refused_instead_of_falling_back() {
    let home = scratch("dangling-home");
    let root = scratch("dangling-env");
    let list = write_one_entry_list(&home, "https://example.invalid", "sk", "m", 1024);
    write_binding(&root, "ms-9");

    let error = model_source::decide(&root, Some(&list), &no_flags(), &[])
        .expect_err("绑的条目不在就该拒绝");
    assert!(error.contains("ms-9"), "{error}");
    assert!(
        error.contains("已被删除"),
        "要说清是「那条没了」 —— {error}"
    );
    // 负向对照：错误里不能出现"改用进程侧 / 回落"这种兜底语义。
    assert!(!error.contains("改用"), "{error}");
}

/// 判据 10：绑定在场 + 进程侧也在说话（这里用旗标那一边）⇒ 拒绝，两边都点出来。
#[test]
fn a_binding_plus_a_process_side_declaration_is_refused() {
    let home = scratch("binding-flags-home");
    let root = scratch("binding-flags-env");
    let list = write_one_entry_list(&home, "https://example.invalid", "sk", "m", 1024);
    write_binding(&root, "ms-1");

    let error = model_source::decide(
        &root,
        Some(&list),
        &FlagDeclarations {
            endpoint: Some("https://other.invalid/v1/chat"),
            ..no_flags()
        },
        &[],
    )
    .expect_err("绑定在场时进程侧必须一言不发");
    assert!(error.contains("ms-1"), "{error}");
    assert!(error.contains("--model-endpoint"), "{error}");
}

/// 判据 11（唤醒边界）：绑定被摘掉、老文件也不在场 ⇒ **如实失败**，不复用上一份。
///
/// 这是"两轮之间发生了什么"的判据：摘掉配置之后自己下一次活动就该报"没有模型来源了"。
#[test]
fn reread_without_any_source_fails_instead_of_reusing_the_last_one() {
    let root = scratch("reread-empty");
    let error = model_source::reread(&root, None).expect_err("一拍没有模型来源就该如实失败");
    assert!(error.contains("绑定"), "{error}");
    // 负向对照：不能报成"读不出来"（那是另一回事），要说清是**没有来源**。
    assert!(error.contains("没有模型来源"), "{error}");
}

// ---------------------------------------------------------------------------
// 判据 12–14：**思考强度那一格**（2026-10-08，第三腿）
// ---------------------------------------------------------------------------
//
// 唯一读侧就是这里：认得的取值收下、认不得的**拒绝**（点名那一件、列出认得的取值）、
// 而**显式 `null` 与"没写这一格"同义**。

/// 写一份**只带这一条**的清单，`extra` 原样插进那一条里（空串 = 没有额外字段）。
fn write_list_with_extra(home: &Path, extra: &str) -> PathBuf {
    let file = home.join("model_sources.json");
    let text = format!(
        "[\n  {{\n    \"id\": \"ms-1\",\n    \"name\": \"默认\",\n    \
         \"endpoint\": \"https://example.invalid/v1/chat\",\n    \"api_key\": \"sk-from-list\",\n    \
         \"model\": \"listed-model\",\n    \"context_window\": 65536{extra}\n  }}\n]\n"
    );
    fs::write(&file, text).expect("写清单");
    file
}

/// 判据 12：认得的取值 ⇒ 原样收下（**取值就是封闭表里那一个**，不做大小写/别名的宽松匹配）。
#[test]
fn a_known_reasoning_effort_is_read_verbatim() {
    let home = scratch("effort-known");
    let root = scratch("effort-known-env");
    let list = write_list_with_extra(&home, ",\n    \"reasoning_effort\": \"high\"");
    write_binding(&root, "ms-1");

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[]).expect("读得成");
    let Source::File(file) = &source else {
        panic!("绑定了清单里的一条就该以文件为源");
    };
    assert_eq!(file.reasoning_effort.as_deref(), Some("high"));
}

/// 判据 13：**认不得的取值 ⇒ 明确拒绝**（点名那一件、列出认得的取值）。
///
/// ⚠ 这里用 `ultra`（**表外**的取值）——`max` 2026-10-09 起**在表里**（主人点名要它，同日实测
/// 支撑：那家网关认它且真有含义，见 `REASONING_EFFORTS` 的注释）。
#[test]
fn an_unknown_reasoning_effort_is_refused_with_the_known_values() {
    let home = scratch("effort-unknown");
    let root = scratch("effort-unknown-env");
    let list = write_list_with_extra(&home, ",\n    \"reasoning_effort\": \"ultra\"");
    write_binding(&root, "ms-1");

    let error = model_source::decide(&root, Some(&list), &no_flags(), &[])
        .expect_err("认不得的取值就该拒绝");
    assert!(error.contains("reasoning_effort"), "{error}");
    assert!(error.contains("ms-1"), "要点名是哪一件 —— {error}");
    assert!(error.contains("ultra"), "要点名是哪个取值 —— {error}");
    for known in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        assert!(
            error.contains(known),
            "要列出认得的取值（缺 {known}）—— {error}"
        );
    }
    // 负向对照：不许静默回落成"没声明"（那会让操作者以为这一档生效了）。
    assert!(!error.contains("改用"), "{error}");

    // 形状不对（不是字符串、也不是 `null`）同样拒绝。
    let wrong_type = write_list_with_extra(&home, ",\n    \"reasoning_effort\": 5");
    let error = model_source::decide(&root, Some(&wrong_type), &no_flags(), &[])
        .expect_err("形状不对就该拒绝");
    assert!(error.contains("reasoning_effort"), "{error}");
}

/// 判据 13b：**`max` 收下**（2026-10-09 表加宽的那一格 —— 主人点名要它）。
///
/// 它同时是"加宽表不需要迁移任何已存数据"那条的落点：清单里存的本来就是原值。
#[test]
fn the_max_tier_is_accepted_verbatim() {
    let home = scratch("effort-max");
    let root = scratch("effort-max-env");
    let list = write_list_with_extra(&home, ",\n    \"reasoning_effort\": \"max\"");
    write_binding(&root, "ms-1");

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[]).expect("max 该收下");
    let Source::File(file) = &source else {
        panic!("绑定了清单里的一条就该以文件为源");
    };
    assert_eq!(file.reasoning_effort.as_deref(), Some("max"));
}

/// 判据 14：**显式 `null` 与"没写这一格"同义**（都不是错误）。
///
/// 为什么单独立一条：JS 那个写者落盘时的规范形状就是 `"reasoning_effort": null`（没填这一格时）。
/// 若把 `null` 判成"形状不对"，**每一次页面保存之后这个家都起不来**——而它说的本来就是
/// "这一格没有值"（与"缺席"同一件事：都不发那个键）。⚠ 这不是放宽封闭表：取值照旧只有那六个。
#[test]
fn an_explicit_null_means_not_declared() {
    let home = scratch("effort-null");
    let root = scratch("effort-null-env");
    let list = write_list_with_extra(&home, ",\n    \"reasoning_effort\": null");
    write_binding(&root, "ms-1");

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[])
        .expect("显式 null 说的是「没有这一格」，不该让这个家起不来");
    let Source::File(file) = &source else {
        panic!("绑定了清单里的一条就该以文件为源");
    };
    assert_eq!(
        file.reasoning_effort, None,
        "没有值 ⇒ 不发那个键（与缺席同义）"
    );
}

/// 原样写一份绑定（加宽形状用；`source` 必填、另两格可选）。
fn write_binding_json(root: &Path, body: &str) {
    fs::write(model_source::binding_path(root), format!("{body}\n")).expect("写绑定");
}

/// 判据 15：**绑定能覆盖"用什么想"**（2026-10-09 主人裁 B：钥匙属机器，型号与档位属她）。
///
/// 为什么要有它：条目说"这台机器上有哪些钥匙"（机器级），而"她要用哪个型号、什么档位"是**她**的
/// 属性 —— 换执行端时她还是她。判据：绑定里写了那两格 ⇒ **它赢**；条目里那两格只是缺省。
#[test]
fn her_binding_wins_over_the_machines_entry_for_model_and_effort() {
    let home = scratch("binding-wins");
    let root = scratch("binding-wins-env");
    let list = write_list_with_extra(&home, ",\n    \"reasoning_effort\": \"low\"");
    write_binding_json(
        &root,
        r#"{ "source": "ms-1", "model": "her-own-model", "reasoning_effort": "max" }"#,
    );

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[]).expect("该收下");
    let Source::File(file) = &source else {
        panic!("绑定了清单里的一条就该以文件为源");
    };
    assert_eq!(file.model, "her-own-model", "型号以她说的为准");
    assert_eq!(
        file.reasoning_effort.as_deref(),
        Some("max"),
        "档位以她说的为准"
    );
    assert_eq!(
        file.entry_id.as_deref(),
        Some("ms-1"),
        "来源仍是那一条（钥匙还是机器的）"
    );
}

/// 判据 16：**没有那两格 ⇒ 用条目里的**（旧数据一字不动、不需要迁移）。
#[test]
fn an_old_binding_keeps_using_the_entry() {
    let home = scratch("binding-old");
    let root = scratch("binding-old-env");
    let list = write_list_with_extra(&home, ",\n    \"reasoning_effort\": \"low\"");
    write_binding(&root, "ms-1");

    let source = model_source::decide(&root, Some(&list), &no_flags(), &[]).expect("该收下");
    let Source::File(file) = &source else {
        panic!("绑定了清单里的一条就该以文件为源");
    };
    assert_eq!(file.model, "listed-model", "条目里那一格照旧生效");
    assert_eq!(
        file.reasoning_effort.as_deref(),
        Some("low"),
        "条目里那一格照旧生效"
    );
}

/// 判据 17：绑定里那两格**形状不对就拒绝**（把"配置坏了"说成"没配"是另一回事）。
#[test]
fn a_malformed_override_is_refused_and_named() {
    let home = scratch("binding-bad");
    let root = scratch("binding-bad-env");
    let list = write_list_with_extra(&home, "");

    write_binding_json(&root, r#"{ "source": "ms-1", "model": 5 }"#);
    let error =
        model_source::decide(&root, Some(&list), &no_flags(), &[]).expect_err("形状不对就拒绝");
    assert!(error.contains("model"), "{error}");
    assert!(error.contains("绑定文件"), "要点名是哪一份文件 —— {error}");

    // 档位照旧走**封闭表**：认不得的取值不许溜进请求体。
    write_binding_json(
        &root,
        r#"{ "source": "ms-1", "reasoning_effort": "ultra" }"#,
    );
    let error =
        model_source::decide(&root, Some(&list), &no_flags(), &[]).expect_err("表外的值就拒绝");
    assert!(error.contains("reasoning_effort"), "{error}");
    assert!(error.contains("ultra"), "{error}");
}
