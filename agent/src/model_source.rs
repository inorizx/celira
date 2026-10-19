//! 环境根里的**模型配置门**（TASK_77；刀2b 起分成"清单 + 绑定"两个事实）。
//!
//! 模型凭据是执行端的**私有事实**；而"这台机器上有哪些钥匙"与"**这一个 Agent 用哪一条**"
//! 是**两个**事实，住在两个地方：
//!
//! | 事实 | 归属 | 文件 |
//! |---|---|---|
//! | 这台机器上有哪些钥匙（**清单**） | **机器** | `<家>/model_sources.json` |
//! | 这一个 Agent 用哪一条（**绑定**） | **Agent** | `<环境根>/model_binding.json` |
//!
//! 一句话原理：**钥匙串归房子，用哪把钥匙归住户**。四件（端点 / 密钥 / 模型名 / 标称窗口，
//! + 可选线协议）在两个之上解析出来。
//!
//! 几句纪律就是这一件的全部：
//!
//! 1. **一个权威（每件事各一个）**：清单条目的形状归写侧（`model_sources.js` 的
//!    `normalizeEntries`），绑定与四件套的形状归本模块；本模块是**唯一读侧**——写侧有两条
//!    通道（页面 `scripts/home/config-server.js` 与无头启动 `home.js`），它们写同一份权威，
//!    不是两份配置。本模块**不缓存、不派生第三份**（`read_entry` 每次现读清单）。
//!    明文，不加密：暴露面与环境变量同级——同一张盘上的加密钥匙保护不了自己。
//! 2. **不许两个来源同时说话**：绑定在场时，**四条声明旗标**里再出现任何模型来源 ⇒
//!    **拒绝**；绑定与**老的单份配置**同时在场同样是双权威 ⇒ **拒绝**——一律把是哪两个打架
//!    说清楚，不猜、不静默覆盖。这与「im 端点与凭据必须成对」是同一条纪律。
//!    ⚠ **凭据环境变量不在"谁在说话"的名单里**（2026-10-08 主人裁决：配置模型是主体自己的
//!    事情）：绑定在场时它不被读，只被如实报一行"已被这一份取代"。
//! 3. **唤醒边界重读**：文件为源时，宿主在**两次活动之间**重装配 ⇒ 下一次活动用新配置；
//!    正在跑的活动绝不受打扰（重装配只发生在上一次跑完之后、下一次开始之前）。
//!
//! **老形状**（`<环境根>/model_source.json`，四件套）仍可读——那是迁移前的现役语义。老家
//! 第一次开机时 `up` 会把它迁成"清单第一条 + 绑定指向它"，并把老文件**改名留档**
//! （`<名>.migrated`）；本模块只认那个**准确文件名**，留档件不是权威。
//! ⚠ **判据是准确文件名，不是前缀**：留档件就叫 `model_source.json.migrated`——用前缀认
//! "老文件还在不在"，迁完之后就再也起不来（它说的是"迁完了"，不是"还在场"）。
//!
//! 它**不是公共契约**：键名只在本模块与那两个写者之间对齐，改动只影响本机的几个进程；
//! 对 Celira 的公共世界（im 服务端）与浏览器，它全程零知情。
//! 它**不发请求、不起进程、不写任何文件**——只读这几份，读不出来就如实失败。

use std::path::{Path, PathBuf};

/// 环境根下的私有文件名。
///
/// 写侧（`scripts/home/config-server.js`）与读侧（本模块）各有一份这个名字——它是**同一个
/// 事实的两个落点**，改一处就得改两处。名字本身不是秘密：它只是"这一格归模型配置"。
pub const FILE_NAME: &str = "model_source.json";

/// **绑定**文件名（刀2b）：这个 Agent 用**清单**里的哪一条。住在**环境根**下。
///
/// 清单（机器级，key 的池子）与绑定（Agent 级，"用哪一条"）分居两处 —— 原理那句话：
/// **钥匙串归房子，用哪把钥匙归住户**。
pub const BINDING_FILE: &str = "model_binding.json";

/// 老的单份配置文件名（四件套形状）——迁移的**输入**；迁移之后它会被改名留档，
/// 本模块再看到它与绑定同时在场，就是**双权威**（拒绝，见 [`decide`]）。
pub const LEGACY_FILE: &str = FILE_NAME;

/// 文件里那四件 + 可选协议的键名（与写侧**逐字对齐**）。
///
/// 四件是换家档的形状：端点 / 密钥 / 模型名 / 标称窗口；协议是可选的第五件（缺省 = 本家那一条）。
const KEY_ENDPOINT: &str = "endpoint";
const KEY_CREDENTIAL: &str = "api_key";
const KEY_MODEL: &str = "model";
const KEY_WINDOW: &str = "context_window";
const KEY_PROTOCOL: &str = "protocol";
/// **思考强度**那一格的键名（2026-10-08，可选）。
const KEY_REASONING: &str = "reasoning_effort";
/// **这一份模型收不收得到媒体材料**那一格的键名（2026-10-17，可选）。
///
/// ⚠ **它是宿主的声明，不是我们探测的**（与 `context_window` 同族）：我们不嗅探、不按模型名猜
/// ——猜错的方向是「工具面说读得动、而请求根本发不出去」。
const KEY_VISION: &str = "vision";

/// **思考强度的封闭表**：请求体上那个通用字段 `reasoning_effort`（本家那条 wire 用的那一套）。
///
/// ⚠ 它是**封闭**的，而且这里是**唯一判据**：认不得的值**明确拒绝**——绝不原样透传、也不静默降级。
/// ⚠ **`max` 在表里**（2026-10-09 主人点名要它，同日实测支撑）：opencode go 的 zen 网关**认它**
/// 而且**真有含义**——同一句话、同一个型号，只改这一格：`none` ⇒ `completion_tokens` 只有 3、
/// **连 `reasoning_tokens` 那一格都没有**（完全不推理）；`high` ⇒ 112（其中 108 是推理）；
/// `max` ⇒ 287（其中 283 是推理）；**不发这一格** ⇒ 179（其中 175）＝网关的缺省档。
/// （原判"`max` 是别家产品的变体词"因此**按实测更正**：在这一家它是一个有效的档。）
/// ⚠ 表**随时可以加宽，且不需要迁移任何已存数据**——清单里存的本来就是原值（与 `protocol` 同款）。
/// ⚠ **不建"每个模型支持哪些档"的能力表**：那是外部事实（会腐烂，且权威不在我们手里）；
/// "这个模型吃不吃"由网关裁决，报错就按模型层失败如实归类。
const REASONING_EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 校验一件思考强度声明（**两处读法共用同一份判据**，`where_` 只说"这是哪一份里的"）。
fn check_reasoning_effort(value: &str, where_: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if REASONING_EFFORTS.contains(&trimmed) {
        return Ok(trimmed.to_string());
    }
    Err(format!(
        "{where_} 的 {KEY_REASONING} 认不得：`{trimmed}`。认得的取值：{}（`max` 不在其中——那是别家的变体词）",
        REASONING_EFFORTS.join(" / ")
    ))
}

/// 文件读出来的**四件套**（换家档形状）。
///
/// ⚠️ `credential` 在这里**只是一个值**：本模块不打印它、不落盘、不给它任何输出通道。
/// 唯一读它的地方是装配那一处（入口把值交给模型层），横幅只报**键名**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSource {
    /// 就是从哪儿读来的（读数用，含绝对路径）。
    ///
    /// - **老形状**（迁移前）：就是那个四件套文件自己的路径；
    /// - **绑定档**（刀2b）：**清单**文件的路径（四件套是从清单的那一条里解析出来的）。
    pub path: PathBuf,
    /// 这一份是从清单的**哪一条**解析出来的（老形状没有这一格 ⇒ `None`）。
    pub entry_id: Option<String>,
    pub endpoint: String,
    pub credential: String,
    pub model: String,
    pub context_window: u32,
    /// 线协议（原始取值；闭集校验在装配入口那一处——那里是协议名的唯一权威）。
    pub protocol: Option<String>,
    /// **思考强度**（2026-10-08，可选）：请求体上那个通用字段 `reasoning_effort`。
    ///
    /// `None` = **不发那个键**（由供应商与模型自己决定），与 `Some("none")`（**明确要求不思考**）
    /// 是**两件事**。取值在**这里**按[`REASONING_EFFORTS`]校验——认不得的到不了下游。
    pub reasoning_effort: Option<String>,
    /// **这一份模型收不收得到媒体材料**（2026-10-17，可选）。
    ///
    /// `false` = **不支持**（不写这一格就是不支持：**fail-closed**——工具面因此不摆出「读得动
    /// 图片」这句话）。它是**宿主的声明**（与 `context_window` 同族），本模块**不探测、不推断**。
    ///
    /// 谁读它：读侧那两件动词（`im_read_attachment` / `files_read`）的**可读类型清单**由它派生
    /// ——"这个模型收不到图像块"时，工具面不该说读得动。
    pub vision: bool,
    /// **被这一份取代的那条进程侧凭据变量的名字**（2026-10-08 主人裁决）。
    ///
    /// `Some(name)` = 这个进程的环境里那条变量**非空**，而它的四件套**不参与**这次装配
    /// （绑定/文件说了算）⇒ 横幅要**如实报一行**，让人知道"它没被用"（人不许比系统知道得更少）。
    /// `None` = 没有这条遗留。⚠ 只存**名字**，**永不存取值**。
    pub superseded_credential: Option<String>,
}

/// 这一次装配的**生效来源**。
///
/// ⚠ **文件那一支装箱**（2026-10-08）：`FileSource` 有九格（六条字符串 + 路径 + 窗口 + 可选的
/// 三格），而进程侧那一支一个字节都不带 —— 思考强度那一格落地时它正好越过 clippy 的
/// `large_enum_variant` 线（**实测 208 字节**，线是 200）。装箱是 clippy 自己给的那条建议：
/// 这个值一条进程读几次（开机一次 + 每次唤醒边界重读一次），一次间接跳转换掉一个 208 字节的
/// 枚举体。**判据没有变**：`Source` 仍然是"听谁的"这一个二选一。
#[derive(Debug, Clone)]
pub enum Source {
    /// 进程环境 / 命令行旗标（现状：默认档或换家档）。
    Process,
    /// 环境根的私有配置文件。
    File(Box<FileSource>),
}

impl Source {
    /// 来源名（读数用的稳定字符串）。
    pub fn origin(&self) -> &'static str {
        match self {
            Source::Process => "process",
            Source::File(_) => "file",
        }
    }

    /// **这一次装配用的模型收不收得到媒体材料**（2026-10-17）。
    ///
    /// 与 `protocol` / `reasoning_effort` 同一处口径：**文件为源时听文件的**（那一格是宿主的
    /// 声明），进程侧来源**没有这一格** ⇒ `false`。
    ///
    /// **fail-closed**：不声明就是不支持——工具面因此不摆出「读得动图片」那句话。两个入口
    /// （常驻宿主与一次性）都读这一处，**不各写一份**。
    pub fn vision(&self) -> bool {
        match self {
            Source::Process => false,
            Source::File(file) => file.vision,
        }
    }
}

/// 进程侧在命令行上声明的模型来源（旗标那四件；原始取值）。
pub struct FlagDeclarations<'a> {
    pub endpoint: Option<&'a str>,
    pub model: Option<&'a str>,
    pub context_window: Option<&'a str>,
    pub protocol: Option<&'a str>,
}

/// 配置文件的绝对路径。
pub fn file_path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

/// 文件在不在场（判据只看它是不是一个文件）。
///
/// **在场就是"以文件为源"**：这与"文件内容能不能读出来"是两件事——前者判"听谁的"，
/// 后者判"读不读得成"。混成一件事会让"文件在但坏了"静默滑回进程侧（那正是本刀要堵的）。
pub fn file_present(root: &Path) -> bool {
    std::fs::metadata(file_path(root))
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// 读并校验文件。
///
/// 三种失败分开说（"读不到/读不出/形状不对"是三种不同的处境），**一律如实失败、
/// 绝不回落进程侧**：文件在场就已经决定了"听它的"，读不出来是配置坏了，不是"改用另一条路"。
pub fn read_file(root: &Path) -> Result<FileSource, String> {
    let path = file_path(root);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "模型配置文件读不出来：{}（{error}）。它坏了不会自动改用进程侧——两边都得看清楚。",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "模型配置文件不是合法 JSON：{}（{error}）。本命令不打印它的内容。",
            path.display()
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        format!(
            "模型配置文件的最外层是一个对象：{}（读到的不是）。",
            path.display()
        )
    })?;

    let field = |key: &str| -> Result<String, String> {
        match object.get(key) {
            Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
            Some(serde_json::Value::String(_)) | None => Err(format!(
                "模型配置文件缺一件（或为空）：{key}。换家档四件必须全显式——{}",
                path.display()
            )),
            Some(_) => Err(format!(
                "模型配置文件里的 {key} 不是字符串：{}",
                path.display()
            )),
        }
    };
    let endpoint = field(KEY_ENDPOINT)?;
    let credential = field(KEY_CREDENTIAL)?;
    let model = field(KEY_MODEL)?;
    let context_window = match object.get(KEY_WINDOW) {
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .and_then(|raw| u32::try_from(raw).ok())
            .filter(|window| *window > 0),
        _ => None,
    }
    .ok_or_else(|| {
        format!(
            "模型配置文件里的 {KEY_WINDOW} 必须是正整数（它是声明，不是实测）：{}",
            path.display()
        )
    })?;
    let protocol = match object.get(KEY_PROTOCOL) {
        None => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(_) => {
            return Err(format!(
                "模型配置文件里的 {KEY_PROTOCOL} 不是字符串（不给这一件就按本家那一条走）：{}",
                path.display()
            ));
        }
    };
    let reasoning_effort = match object.get(KEY_REASONING) {
        None => None,
        // **显式 `null` 与"没写这一格"同义**（2026-10-08 实测：JS 那个写者落盘时写的是
        // `"reasoning_effort": null`）。它是 JSON 里说"这一格没有值"的自然写法，而"没有值"
        // 与"缺席"是同一件事（都不发那个键）⇒ 认它。⚠ 这不是放宽封闭表：**取值**照旧只有
        // 表里那六个，别的形状（数字 / 对象 / 数组）一律拒绝。
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Some(
            check_reasoning_effort(text, &format!("模型配置文件（{}）", path.display()))?,
        ),
        Some(_) => {
            return Err(format!(
                "模型配置文件里的 {KEY_REASONING} 不是字符串（不给这一件就不发那个键）：{}",
                path.display()
            ));
        }
    };

    // 见 `FileSource::vision`：**不写就是不支持**（fail-closed）。
    let vision = match object.get(KEY_VISION) {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(value)) => *value,
        Some(_) => {
            return Err(format!(
                "模型配置文件里的 {KEY_VISION} 必须是 true / false（不写这一格就是不支持）：{}",
                path.display()
            ));
        }
    };

    Ok(FileSource {
        path,
        // 老形状没有"哪一条"这个概念。
        entry_id: None,
        endpoint,
        credential,
        model,
        context_window,
        protocol,
        // 见 `FileSource::reasoning_effort`。
        reasoning_effort,
        vision,
        // 老形状那一份**不知道**进程侧有什么（那不是它的读面）：那一格由 `decide` 填。
        superseded_credential: None,
    })
}

/** 绑定文件的路径（环境根下）。 */
pub fn binding_path(root: &Path) -> PathBuf {
    root.join(BINDING_FILE)
}

/**
 * 绑定（**支级事实**：这一支用清单里的哪一条）。
 *
 * ⚠ **2026-10-09 加宽**（主人裁：钥匙属机器，"用什么型号/什么档位"归**主体**）：
 * 清单那一条说"这台机器上有哪些钥匙"（**机器级**），而 `model` / `reasoning_effort` 这两格
 * 允许写进**绑定**（**支级**）来覆盖条目里的同名字段 ⇒ **换执行端，主体不变**。
 * 两格都是可选的：**不写 ⇒ 用条目里那个**（旧机器上那份绑定不带它们 ⇒ 行为一字不变 ⇒
 * **不需要迁移任何数据**）。形状不对一律**如实失败**（把"配置坏了"说成"没配"是另一回事）。
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// 清单里的哪一条（必填；空串 = 没有 ⇒ 拒绝）。
    pub source: String,
    /// **这一支要用哪个型号**（可选；覆盖条目里那一格）。
    pub model: Option<String>,
    /// **这一支要用哪个思考强度**（可选；覆盖条目里那一格，取值仍按 [`REASONING_EFFORTS`] 校验）。
    pub reasoning_effort: Option<String>,
}

/**
 * 读绑定：`Ok(None)` = 没有绑定（合法状态，走进程侧或老形状）；`Ok(Some(binding))` = 绑到哪一条。
 *
 * 读不出来 / 形状不对 ⇒ **如实失败**（不当作"没绑"：那会把"配置坏了"说成"没配"，
 * 于是主体去走 env 那一路，而操作者以为一切照旧）。
 */
pub fn read_binding(root: &Path) -> Result<Option<Binding>, String> {
    let path = binding_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("绑定文件读不出来：{}（{error}）", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("绑定文件不是合法 JSON：{}（{error}）", path.display()))?;
    let where_ = format!("绑定文件（{}）", path.display());
    let source = value
        .get("source")
        .and_then(|it| it.as_str())
        .map(|it| it.trim().to_string())
        .filter(|it| !it.is_empty());
    let Some(source) = source else {
        return Err(format!(
            "绑定文件里没有 source（它是 {{\"source\": \"<清单条目 id>\"}} 这个形状）：{}",
            path.display()
        ));
    };
    // **可选的两格**（2026-10-09 加宽）：给了就必须是合规的字符串；`null` 与"没写"同义
    // （JS 写者的规范形状就是写 `null` —— 与清单那两格同一条口径）。
    let model = match value.get(KEY_MODEL) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
            Some(text.trim().to_string())
        }
        Some(_) => {
            return Err(format!(
                "{where_} 里的 model 要么不写、要么是一个非空字符串（它是\"这一支用哪个型号\"）"
            ));
        }
    };
    let reasoning_effort = match value.get(KEY_REASONING) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
            Some(check_reasoning_effort(text.trim(), &where_)?)
        }
        Some(_) => {
            return Err(format!(
                "{where_} 里的 reasoning_effort 要么不写、要么是表里的一个取值"
            ));
        }
    };
    Ok(Some(Binding {
        source,
        model,
        reasoning_effort,
    }))
}

/// **把绑定里那两格覆盖到条目那一份上**（缺 ⇒ 一个字都不动）。
///
/// 判据一句话：**钥匙属机器，用什么想归主体**。条目（机器级）说"这台机器上有哪些钥匙、
/// 默认用哪个型号"；绑定（支级）说"**主体**要用哪个型号、什么档位" ⇒ 后者赢。
fn apply_binding_overrides(file: &mut FileSource, binding: &Binding) {
    if let Some(model) = &binding.model {
        file.model = model.clone();
    }
    if binding.reasoning_effort.is_some() {
        file.reasoning_effort = binding.reasoning_effort.clone();
    }
}

/**
 * 在**清单**里按 id 解析出四件套（刀2b）。
 *
 * 三种失败分开说，**都不静默**：
 * - 清单文件读不出来 / 不是 JSON / 不是数组 ⇒ 如实报路径与原因；
 * - 清单里**没有那个 id** ⇒ 明说"绑定的模型源已被删除"（这是悬空绑定，`decide` 会拒绝启动）；
 * - 那一条本身缺件 / 窗口不是正整数 ⇒ 点名是哪一件。
 */
pub fn read_entry(list_path: &Path, id: &str) -> Result<FileSource, String> {
    let text = std::fs::read_to_string(list_path).map_err(|error| {
        format!(
            "模型源清单读不出来：{}（{error}）。绑定要配清单才能解析 —— \
             `node home.js up` 会把清单路径交给本进程（`--model-sources`）。",
            list_path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "模型源清单不是合法 JSON：{}（{error}）",
            list_path.display()
        )
    })?;
    let entries = value.as_array().ok_or_else(|| {
        format!(
            "模型源清单的最外层是一个数组：{}（读到的不是）。",
            list_path.display()
        )
    })?;
    let entry = entries
        .iter()
        .find(|each| each.get("id").and_then(|it| it.as_str()) == Some(id))
        .ok_or_else(|| {
            format!(
                "绑定的模型源已被删除：清单 {} 里没有 id = {id} 这一条。\
                 去「模型配置」页换绑一条，或把绑定摘掉（回到环境变量那一路）。",
                list_path.display()
            )
        })?;
    let field = |key: &str| -> Result<String, String> {
        match entry.get(key) {
            Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
            _ => Err(format!(
                "清单里 {id} 这一条缺一件（或为空）：{key}（四件必须全在）"
            )),
        }
    };
    let endpoint = field(KEY_ENDPOINT)?;
    let credential = field(KEY_CREDENTIAL)?;
    let model = field(KEY_MODEL)?;
    let context_window = match entry.get(KEY_WINDOW) {
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .and_then(|raw| u32::try_from(raw).ok())
            .filter(|window| *window > 0),
        _ => None,
    }
    .ok_or_else(|| {
        format!("清单里 {id} 这一条的 {KEY_WINDOW} 必须是正整数（它是声明，不是实测）")
    })?;
    let protocol = match entry.get(KEY_PROTOCOL) {
        None => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(_) => {
            return Err(format!(
                "清单里 {id} 这一条的 {KEY_PROTOCOL} 不是字符串（不给这一件就按本家那一条走）"
            ));
        }
    };
    let reasoning_effort = match entry.get(KEY_REASONING) {
        None => None,
        // **显式 `null` 与"没写这一格"同义**（2026-10-08 实测：JS 那个写者落盘时写的就是
        // `"reasoning_effort": null`）。若把 `null` 判成"形状不对"，**每一次页面保存之后这个家
        // 都起不来**——而它说的本来就是"这一格没有值"。⚠ 取值照旧只有封闭表里那六个。
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Some(
            check_reasoning_effort(text, &format!("清单里 {id} 这一条"))?,
        ),
        Some(_) => {
            return Err(format!(
                "清单里 {id} 这一条的 {KEY_REASONING} 不是字符串（不给这一件就不发那个键）"
            ));
        }
    };
    // 见 `FileSource::vision`：**不写就是不支持**（fail-closed）。
    let vision = match entry.get(KEY_VISION) {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(value)) => *value,
        Some(_) => {
            return Err(format!(
                "清单里 {id} 这一条的 {KEY_VISION} 必须是 true / false（不写这一格就是不支持）"
            ));
        }
    };
    Ok(FileSource {
        path: list_path.to_path_buf(),
        entry_id: Some(id.to_string()),
        endpoint,
        credential,
        model,
        context_window,
        protocol,
        reasoning_effort,
        vision,
        // 清单里那一条**不知道**进程侧有什么：那一格由 `decide` 填（它才看得见"谁在说话"）。
        superseded_credential: None,
    })
}

/// 「进程侧在说话」吗？返回**人话的来源名**（冲突读数用）。
///
/// **只算旗标**（2026-10-08 主人裁决：**配置模型是主体自己的事情**）。
///
/// 四个声明旗标是**四件套里的那几件**（端点 / 模型名 / 窗口 / 协议）：文件或绑定在场时，它们
/// 说的是**同一件东西** ⇒ 两边同时说 ⇒ **拒绝**（真歧义：开不了机比静默选边好）。
///
/// ⚠ **凭据环境变量不算**（原样撤回 2026-10-08）：它只是"启动这个进程的环境里恰好有那把钥匙"，
/// 与绑定声明的四件套**不重叠**（清单里那一条自带钥匙），而且**绑定在场时它永不被读** ⇒
/// 让它否决主体自己的配置，就是把开发期的兜底当成了配置的一档。⇒ 从"谁在说话"的名单里拿出来，
/// 改由[`credential_env_present`]如实报一行"它被取代了"。
pub fn declared_by_flags(flags: &FlagDeclarations) -> Option<String> {
    [
        ("--model-endpoint", flags.endpoint),
        ("--model", flags.model),
        ("--context-window", flags.context_window),
        ("--model-protocol", flags.protocol),
    ]
    .into_iter()
    .find(|(_, value)| value.map(|text| !text.trim().is_empty()).unwrap_or(false))
    .map(|(name, _)| format!("命令行 {name}"))
}

/// **进程侧有哪一条凭据变量非空**（名字由**调用方**传进来：这一格是给所有云厂商用的通用门，
/// 而"我们认哪几条变量"属于入口的装配事实——本模块因此不必认识任何一家厂商的名字）。
///
/// 它**不再参与"谁在说话"的裁决**（见 [`declared_by_flags`]），只用来产出一行如实读数：
/// 那条变量**被这一份取代了、没有参与这次装配**。返回的只是**名字**，取值一个字节都不碰。
pub fn credential_env_present(credential_envs: &[&str]) -> Option<String> {
    credential_envs
        .iter()
        .find(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
        .map(|name| name.to_string())
}

/// 这一次装配听谁的（刀2b：**绑定 → 清单**；老形状仍可读）。
///
/// 判据顺序（每一步都能单独测）：
///
/// 1. **绑定不在场**：
///    - 老形状 `<环境根>/model_source.json` 在场 ⇒ [`Source::File`]（**迁移前的现役语义**：
///      那一份就是四件套。现状分毫不动。老家第一次开机时 `up` 会先把它迁掉，此后走不到这里）；
///      它与进程侧**同样不许同时说话**——**模型旗标**在说话 ⇒ **拒绝**（同一条纪律，TASK_77 原样；
///      2026-10-08 起**凭据环境变量不算**，见 [`declared_by_flags`]）；
///    - 都没有 ⇒ [`Source::Process`]（env / 换家档那一路）。
/// 2. **绑定在场**（`Ok(Some(id))`）：
///    - **老形状也残留** ⇒ **拒绝**（双权威：先跑迁移，迁完老文件会被改名留档）；
///    - **清单路径没声明**（缺 `--model-sources`） ⇒ **拒绝**（绑定没有清单解析不出来，说清）；
///    - **模型旗标也在说话** ⇒ **拒绝**，说清是哪两个打架；⚠ **凭据环境变量不拒绝**
///      （2026-10-08：配置模型是主体自己的事情）——它记进 [`FileSource::superseded_credential`]，
///      由横幅如实报一行"它没被用"；
///    - 清单里**找不到那个 id** ⇒ **拒绝**（悬空绑定："绑定的模型源已被删除"——不静默回落 env、
///      也不沿用上一份）；
///    - 否则 ⇒ [`Source::File`]（四件套从清单那一条解析出来）。
///
/// `credential_envs` 是入口认的那两条模型凭据变量名（只用来判"那边在不在说话"）。
pub fn decide(
    root: &Path,
    list_path: Option<&Path>,
    flags: &FlagDeclarations,
    credential_envs: &[&str],
) -> Result<Source, String> {
    let Some(binding) = read_binding(root)? else {
        // ---- 没有绑定 ----
        if file_present(root) {
            // 老形状（迁移前）也照同一条纪律：**文件在场，进程侧就得一言不发**。
            if let Some(other) = declared_by_flags(flags) {
                return Err(format!(
                    "模型来源有两个，不知听谁的：环境根配置文件（{}）与 {other}。\
                     文件在场时进程侧必须一言不发——清掉那一边（换家档的旗标），\
                     或者先摘掉配置文件（`node home.js down` 之后删掉它）。",
                    file_path(root).display()
                ));
            }
            let mut file = read_file(root)?;
            file.superseded_credential = credential_env_present(credential_envs);
            return Ok(Source::File(Box::new(file)));
        }
        return Ok(Source::Process);
    };
    let id = binding.source.clone();
    // ---- 绑定在场 ----
    if file_present(root) {
        return Err(format!(
            "模型来源有两个，不知听谁的：绑定（{} → {id}）与老的单份配置（{}）。\
             先跑一次迁移（`node home.js up` 会做，幂等）—— 迁完老文件会被改名留档。",
            binding_path(root).display(),
            file_path(root).display()
        ));
    }
    let Some(list_path) = list_path else {
        return Err(format!(
            "绑定了模型源（{id}），但没声明清单在哪：本进程要 `--model-sources <清单路径>`\
             （`node home.js up` 会替你给）。绑定说的是\"这个 Agent 用哪一条\"，\
             清单才是那一条本身。"
        ));
    };
    if let Some(other) = declared_by_flags(flags) {
        return Err(format!(
            "模型来源有两个，不知听谁的：模型源清单（{} → {id}）与 {other}。\
             绑定在场时进程侧必须一言不发——清掉那一边（换家档的模型旗标），\
             或者把绑定摘掉（「模型配置」页里删除配置 = 回到环境变量那一路）。",
            list_path.display()
        ));
    }
    // ⚠ **凭据环境变量不在这里拒绝**（2026-10-08 主人裁决）：它不参与这次装配（四件套来自清单
    // 那一条），只需**如实报一行**让人知道它被取代了。
    let mut file = read_entry(list_path, &id)?;
    // **绑定里那两格赢**（2026-10-09 加宽：钥匙属机器，用什么想归主体）。
    apply_binding_overrides(&mut file, &binding);
    file.superseded_credential = credential_env_present(credential_envs);
    Ok(Source::File(Box::new(file)))
}

/**
 * **唤醒边界重读**（TASK_77 的铁律 + 刀2b 的绑定）：同一种模式、换内容。
 *
 * - 绑定在场 ⇒ 重读**绑定与清单那一条**（清单里那一条被改了参数 = 下一次活动用新的）；
 * - 绑定被摘掉 ⇒ 退回老形状（在场的话），或**如实失败**（"这一轮没有模型来源了"——
 *   不复用上一份：那会让操作者以为配置还在）。
 *
 * 它与 [`decide`] 有一处**有意的不对称**：这里不判"绑定与老文件同时在场"那种双遗迹。
 * 理由是"听谁的"在这一轮本来就**没有歧义**——绑定在场时绑定说了算（老文件只是遗留物，
 * 它已经不再是权威）；而 `decide` 那一处是**开机**，开不了机比静默选边好，所以那里拒绝。
 * 遗留物下一次开机自会被告出。**这是记账，不是漏洞**。
 */
pub fn reread(root: &Path, list_path: Option<&Path>) -> Result<Source, String> {
    let Some(binding) = read_binding(root)? else {
        if file_present(root) {
            return Ok(Source::File(Box::new(read_file(root)?)));
        }
        return Err("绑定已被摘掉，老的单份配置也不在场：这一拍没有模型来源了。\
             去「模型配置」页绑一条，或者重新开机（`node home.js up`）按环境变量那一路起这个 Agent。"
            .to_string());
    };
    let id = binding.source.clone();
    let Some(list_path) = list_path else {
        return Err(format!(
            "绑定了模型源（{id}），但没声明清单在哪：本进程要 `--model-sources <清单路径>`"
        ));
    };
    // **绑定里那两格赢**（与 `decide` 同一处口径：读侧只有一个实现，`apply_binding_overrides`）。
    let mut file = read_entry(list_path, &id)?;
    apply_binding_overrides(&mut file, &binding);
    Ok(Source::File(Box::new(file)))
}

/// 横幅要打的那几行（**只报来源名与键名，永不报取值**；以文件为源时才出场）。///
/// 端点与模型名不是秘密（同一屏上 `--model-endpoint` 本来就直接进命令行），窗口也在既有的
/// `context-window=` 那一行里；凭据那一样**只报键名**——这就是"洗凭据读数"在这里的全部含义。
/// 进程侧为源时返回空：现状零改动（横幅不多一行）。
pub fn banner_lines(source: &Source) -> Vec<String> {
    match source {
        Source::Process => Vec::new(),
        Source::File(file) => {
            let mut lines = vec![
                "model-source=file".to_string(),
                format!("model-file={}", file.path.display()),
            ];
            // 绑定档才有这一格（老形状没有"哪一条"）。
            if let Some(id) = &file.entry_id {
                lines.push(format!("model-source-entry={id}"));
            }
            lines.push(format!("model-endpoint={}", file.endpoint));
            lines.push(format!("model={}", file.model));
            // **生效的线协议**（2026-10-11 补）：与下面 `model-reasoning-effort` 那一条**同一个理由**
            // ——清单里那一格决定**哪一条 wire 真的在跑**，而不报它，操作者就只能对着清单猜。
            // 为什么"没声明"也要报一行：这一格**永远有值**（缺省 = 本家那一条），所以只报有值的那一档
            // 会让"我声明的 responses 到底生效了没有"这个问题继续靠猜——那正是 2026-10-11 实测里
            // 绊住我的地方（清单里那条 `protocol: responses` **确实生效了**，但横幅上看不出来）。
            lines.push(match &file.protocol {
                Some(protocol) => format!("model-protocol={protocol}"),
                None => "model-protocol=（清单里没声明 ⇒ 本家那一条）".to_string(),
            });
            // **生效的思考强度**（2026-10-09）：它可能来自绑定（"主体要用哪一档"）而不是条目
            // ⇒ 如实报一行，免得人对着清单上那一格猜"到底发了哪个"。
            if let Some(effort) = &file.reasoning_effort {
                lines.push(format!("model-reasoning-effort={effort}"));
            }
            // **这一份模型收不收得到媒体材料**（2026-10-17）：与上面 `protocol` 那一条**同一个理由**
            // ——它决定**工具面摆不摆出「读得动图片」那句话**，而不报它，操作者就只能对着清单猜
            // 「我声明了视觉，为什么还是读不了图」。两档都报。
            lines.push(match file.vision {
                true => "model-vision=yes".to_string(),
                false => "model-vision=no（不写这一格就是不支持）".to_string(),
            });
            lines.push(format!("model-credential={KEY_CREDENTIAL}（只报键名）"));
            // **被取代的那条遗留**（2026-10-08）：环境里那条凭据变量非空，但它**不参与**这次装配
            // ⇒ 如实报一行（只报名），免得人以为"我设的那把钥匙还在用"。
            if let Some(name) = &file.superseded_credential {
                lines.push(format!(
                    "model-source-note=进程侧凭据 {name} 在场，已被这一份取代（它不参与这次装配）"
                ));
            }
            lines
        }
    }
}
