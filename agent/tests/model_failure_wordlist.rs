//! 模型失败词表的机械判据（TASK_60）：**编解码对称、封闭、旧词可解、标记形状稳定**。
//!
//! 词表是**事实**（进 `env/facts.ndjson`、进经历段首的 `failure = …`、进恢复报告与宿主
//! stderr 行），所以它的判据必须是机械的：`as_str` 是权威位置、`from_str` 同一刀对称补齐、
//! 认不出的取值**明确拒绝**（fail-closed，不猜、不就近映射）。
//!
//! 为什么值得单独立文件：TASK_58 的现场读数里只有一个词 `transport-failure`，
//! **分不出 DNS / 连接被拒 / TLS / 超时 / 读写中断**——而"主体答不上话"多半就栽在这几档之一。

use celira_agent::core::{HttpStatusDetail, ModelFailureClass, TransportDetail};

/// **词表终稿**：每一个取值与它的稳定标记（`as_str` 的权威表，共 17 格）。
///
/// 这同时是一张"新增取值要改哪里"的清单：往枚举里加一格，就要在这里加一行。
///
/// ⚠ **为什么没有 `transport-failure:tls`**：TLS 握手失败的签名**不稳定**（经代理给
/// `InvalidData`、绕代理给 `Other`，两种都实测到过）⇒ 按"只认能机械判定的档"不发布它，
/// 那种情况归"未细分"（`transport-failure`）。
const WORDLIST: [(ModelFailureClass, &str); 17] = [
    // `Transport`：一档"已知未细分" ＋ 四个细分档
    (ModelFailureClass::Transport(None), "transport-failure"),
    (
        ModelFailureClass::Transport(Some(TransportDetail::Dns)),
        "transport-failure:dns",
    ),
    (
        ModelFailureClass::Transport(Some(TransportDetail::Refused)),
        "transport-failure:refused",
    ),
    (
        ModelFailureClass::Transport(Some(TransportDetail::Timeout)),
        "transport-failure:timeout",
    ),
    (
        ModelFailureClass::Transport(Some(TransportDetail::Io)),
        "transport-failure:io",
    ),
    // `HttpStatus`：一档"已知未细分" ＋ 四个按"下一步行动"分的档
    (ModelFailureClass::HttpStatus(None), "http-status-failure"),
    (
        ModelFailureClass::HttpStatus(Some(HttpStatusDetail::Credential)),
        "http-status-failure:credential",
    ),
    (
        ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RateLimited)),
        "http-status-failure:rate-limited",
    ),
    (
        ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide)),
        "http-status-failure:server-side",
    ),
    (
        ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RequestRejected)),
        "http-status-failure:request-rejected",
    ),
    // 其余七档：没有细分，保持原样
    (ModelFailureClass::UnparsableResponse, "unparsable-response"),
    (ModelFailureClass::ContractViolation, "contract-violation"),
    (ModelFailureClass::StoppedGeneration, "stopped-generation"),
    (
        ModelFailureClass::UnsupportedToolName,
        "unsupported-tool-name",
    ),
    (ModelFailureClass::ReservedToolName, "reserved-tool-name"),
    (ModelFailureClass::UnpairedHistory, "unpaired-history"),
    (ModelFailureClass::ProtocolPayload, "protocol-payload"),
];

#[test]
fn wordlist_round_trips_every_value() {
    for (class, mark) in WORDLIST {
        assert_eq!(class.as_str(), mark, "as_str 与词表不一致");
        assert_eq!(
            ModelFailureClass::from_str(mark),
            Some(class),
            "from_str 必须认回同一格（编解码对称）"
        );
        assert_eq!(
            ModelFailureClass::from_str(class.as_str()),
            Some(class),
            "as_str → from_str 必须回到原格"
        );
    }
}

#[test]
fn wordlist_marks_are_unique_and_shaped_stably() {
    // 判据"同一故障两次发生，这个字符串逐字节相同"的机械表达：
    // 标记只能是**小写字母 / `-` / `:`** —— 没有空白、没有数字、没有路径、没有随机，
    // 于是它天然可重复、可放进一行、可被 `\S+` 的消费方整吃。
    let mut seen = std::collections::BTreeSet::new();
    for (_, mark) in WORDLIST {
        assert!(
            mark.chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c == ':'),
            "标记里有不该出现的字符：{mark}"
        );
        assert!(seen.insert(mark), "标记重复了：{mark}");
    }
    assert_eq!(seen.len(), WORDLIST.len());
}

#[test]
fn old_marks_are_prefixes_of_detailed_marks() {
    // **兼容判据**：没细分的那一形态与 TASK_60 之前**逐字节相同**，
    // 而带细分的是它的前缀扩展 ⇒ 只做前缀匹配的旧消费方也读得通（且不必改）。
    for mark in ["transport-failure", "http-status-failure"] {
        assert_eq!(
            ModelFailureClass::from_str(mark).map(|c| c.as_str()),
            Some(mark)
        );
        let detailed = WORDLIST
            .iter()
            .filter(|(_, candidate)| candidate.starts_with(&format!("{mark}:")))
            .count();
        assert!(detailed > 0, "{mark} 应有带细分的形态");
    }
}

#[test]
fn unknown_marks_are_rejected_fail_closed() {
    for bad in [
        "transport-failure:nope",      // 认不出的细分
        "http-status-failure:nope",    // 同上
        "transport-failure:",          // 空细分
        "unparsable-response:dns",     // 给**没有细分**的档硬加细分
        "transport-failure:dns:extra", // 多一段
        "transport_failure",           // 分隔符写错
        "Transport-Failure",           // 大小写不对
        "",                            // 空串
    ] {
        assert_eq!(
            ModelFailureClass::from_str(bad),
            None,
            "认不出的取值必须明确拒绝（不许就近映射）：{bad}"
        );
    }
}

#[test]
fn wordlist_covers_as_many_marks_as_it_declares() {
    // 门禁不许空跑：这张表的规模本身是被断言的——少一行就是少一个取值。
    assert_eq!(WORDLIST.len(), 17);
    // 细分档的总数（Transport 4 ＋ HttpStatus 4）：少了说明有一档只写在枚举里、没进词表。
    let detailed = WORDLIST
        .iter()
        .filter(|(_, mark)| mark.contains(':'))
        .count();
    assert_eq!(detailed, 8);
}
