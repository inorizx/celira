'use strict'
/**
 * **模型配置门**（TASK_77；刀2b 起管"清单 + 绑定"两个事实）：本机的一个窄写口，
 * 管的是**执行端的私有事实**。
 *
 * 它回答的问题只有一个：**Agent 思考用的那个模型，端点/钥匙/模型名/窗口是什么**。
 * 而这件事在刀2b 之后分成**两个**事实、落在**两个**文件（形状与迁移的唯一权威在
 * `model-sources.js`）：
 *
 * | 事实 | 归属 | 文件 |
 * |---|---|---|
 * | 这台机器上有哪些钥匙（**清单**） | **机器** | `<家>/model_sources.json` |
 * | 这一个 Agent 用哪一条（**绑定**） | **Agent** | `<环境根>/model_binding.json` |
 *
 * 一句话原理：**钥匙串归房子，用哪把钥匙归住户**。明文，不加密：暴露面与环境变量同级——
 * 同一张盘上的加密钥匙保护不了自己。
 *
 * 四条设计理由（这个口的形状全在这里，别漂移）：
 *
 *  1. **一个进程一个身份**：状态口是**只读**的（`status-server.js`），写不许塞进去；
 *     模型门是**这一件事的写口**，因此它自己一个进程。它不做只读读数、不代理 im、不看账本。
 *  2. **口上无凭据**（与状态口同一条纪律）：它的环境是白名单（操作系统必需品 ＋
 *     `CELIRA_HOME_DIR`），家的凭据（`DEEPSEEK_API_KEY` / `CELIRA_IM_BACKEND_AUTH` / token）
 *     一个字节都不进来。key 只在请求处理的内存里过一次：**不进日志、不进响应、不进任何
 *     持久层**（唯一落点就是那两个文件里属于它的那一格）。响应里只说"配没配"（`keyConfigured`）。
 *  3. **同源 + loopback 两道（默认，模式一）**：口子只绑 `127.0.0.1`，**不发任何 CORS 头**
 *     （页面走同源代转，跨源 PUT/DELETE 由浏览器自己的 preflight 拦掉）；再按 `Origin`/`Host`
 *     白名单认一遍来源（只认 `127.0.0.1` / `localhost`）——那是防 DNS rebinding 的第二道，不是装饰。
 *     **家口远程访问刀**加了模式二（`config_bind` 绑非 loopback）：那一档里口子真的出机器了，
 *     于是换一套防护——**家钥匙**（`Authorization: Bearer`，摘要比对）+ TLS + 开放 CORS
 *     （认证只靠钥匙，不做 origin 白名单）。两条 fail-closed、钥匙纪律、CORS 都在
 *     `home-key.js`（那一层是这块的唯一权威）。两模式按绑定地址二选一，**不混搭**。
 *  4. **保存不等于能连**：保存后**试连一次**（`GET <基址>/models`，免费、无副作用），结果
 *     如实回（含 HTTP 状态码级别的读数），还顺手把标称窗口（TASK_77 回炉③）与**模型目录**
 *     （刀2b：让页面能照着挑一个模型名）读回来。
 *     **试连失败不拦保存**——网络瞬态不该拦一个主权决定，但失败原因要如实显示
 *     （把"没连上"画成"配好了"才是撒谎）。
 *
 * 端面**只有五条路径**（穷举是为了让"这个口能干什么"在一屏之内看完）：
 *
 * - `/home-config`：`GET`（**三合一**：清单 + 绑定 + 由绑定算出来的派生字段）、
 *   `POST`（**新建一条**清单条目——id 由门给）、`PUT`（**更新一条**：点名 id，或
 *   一条都不点就更新当前绑定的那一条）、`DELETE`（不带 `id` = 解除绑定；带 `id` = 删条目，
 *   被绑定就拒）；
 * - `/home-binding`：`PUT`（换绑 / 解绑；读绑定走在上面那个 GET 里，别处不开第二条读面）。
 *   `agentId` 给了就是**只给这一支**换；**这一支还没住户化**（还没被接到这台机器上）时，
 *   这一笔落在**它将来会住的那个环境根**上（`<家>/agents/<id>/`）—— **选择先于接上**
 *   （2026-10-14：绑定的落点本来就是"它的环境根"，而环境根要等接它第 2 步才建）。
 *   同一个请求体里还可以带**支级覆盖那两格**（`model` / `reasoningEffort`，2026-10-14）：
 *   "**这一支具体用哪个型号、什么思考档**"——运行时一直认它们
 *   （`agent/src/model_source.rs` 的 `apply_binding_overrides`），而本门是它们的**唯一写者**。
 *   不给 = 不动（沿用旧值），`null` = 清掉。
 * - `/home-config/catalog`：`POST`（**只读目录**，2026-10-14）——"**这条钥匙**能调哪些型号"：
 *   请求体只有条目 `id`，钥匙用清单里**存着的那把**（key 永不回浏览器）。只读、不落盘、
 *   响应里没有 key，读数形状与探测那一格**同一份**。
 * - `/home-config/probe`：`POST`（**只读探测**，2026-09-23 预设供应商层加的）——"用这把钥匙
 *   去那一家问一次模型目录"。`{ endpoint, api_key, protocol?, model? }` 进，试连读数出：
 *   **它不写任何文件**（探测即测试，保存才落盘 —— 主人拍过的裁决），响应里**永远没有 key**。
 *   `protocol` 只做形状预检（探测打的是 `GET <基址>/models`，与线协议无关；线协议到**保存**
 *   那一步才起作用）。
 * - `/home-control/attach`：`POST`（★ 2026-10-15 远程接上那一刀）——**接上一支 Agent**。
 *   `{ agentId }` 进，结局读数出（`homed` / `homed-not-started` / `already-homed` / `failed`
 *   ＋原样读数）。⚠ 它是这扇门上**唯一一条会改变执行端进程**的路（其余几条都只是读写模型配置）：
 *   走这台机器自己的准入（家钥匙 ＋ TLS），而且**另有一道单独的授权**——
 *   `home.conf` 的 `remote_control=on`（**只能在本地打开**）；没开 ⇒ 403 `remote_control_off`。
 *   四条边界与留痕见 [`CONTROL_PATH`] 与 `remote-control.js` 的文件头。
 */

const http = require('node:http')
const https = require('node:https')
const fs = require('node:fs')
const path = require('node:path')

const P = require('./paths')
const { confGet } = require('./conf')
const { EXIT } = require('./codes')
const homeKey = require('./home-key')
const statusServer = require('./status-server')
const modelSources = require('./model-sources')
// **模型配置那两份文件的见证**（2026-10-18）：这道门的每一笔写都留一条（谁最后改过这一格）。
// 缝在这道门与壳那道门上，**不在** `modelSources.writeAtomic` 上 —— 理由见那个模块的文件头。
const modelWitness = require('./model_witness')
const remoteControl = require('./remote-control')
const residents = require('./residents')
// **兜底表**：探测（`GET <基址>/models`）拿不到窗口时在这儿补一手。
// 数据由 `scripts/refresh-model-windows.mjs` 在**发版时**摘录（LiteLLM 的公开表，MIT）、随包发 ⇒ 运行时零网络。
const { loadWindowTable } = require('./model-windows')

/** 模型门默认端口（`home.conf` 的 `config_port` 可覆盖；默认值住在 `paths.js`，与另三个端口同一个家）。 */
const DEFAULT_CONFIG_PORT = P.DEFAULT_CONFIG_PORT

/**
 * 线协议的**闭集**。权威在 Rust 两侧适配层的公开常量（`PROVIDER_ADAPTER::PROTOCOL` /
 * `RESPONSES_ADAPTER::PROTOCOL`）；门侧只做**同语法预检**（防呆），不一致以 Rust 侧拒绝为准。
 */
const PROTOCOLS = ['chat-completions', 'responses']

/** 标称窗口的同语法预检（Rust 侧 `parse_context_window` 是权威：正整数，0 不是一条可解释的声明）。 */
const WINDOW = /^[0-9]{1,10}$/

/** `agentId` 的长度上界（与壳那一侧同一个数）。 */
const AGENT_ID_MAX = 200

/**
 * **`agentId` 的形状**：它必须是一个**名字**（字母数字起头，后面只许字母数字 / `.` / `_` / `-`）。
 *
 * 为什么是白名单而不是黑名单：这一格的值会**变成一个目录名**（`<家>/agents/<id>/`）——
 * 黑名单要同时挡住空白、控制字符、路径分隔符、冒号、`..`、以及各平台自己的怪东西，
 * 漏一个就是一条目录穿越；白名单一个字符集就把它关死了（真身份是 UUID，本来就落在这里）。
 *
 * ⚠ **与壳那一侧逐字同形**（`app/src-tauri/src/home_doors.rs` 的 `agent_id_shape_ok`）：
 * 两个生产者对同一个输入必须给同一个答案。长度按**字节**数（白名单全是 ASCII ⇒ 没有歧义）。
 */
function agentIdShapeOk(value) {
  const id = typeof value === 'string' ? value.trim() : ''
  return id.length <= AGENT_ID_MAX && /^[0-9A-Za-z][0-9A-Za-z._-]*$/.test(id)
}

/**
 * **只读探测**那一条路径（2026-09-23 预设供应商层）。
 *
 * 为什么挂在 `/home-config` 底下而不是另开一个顶层端面：它读/想的是**同一件事**（这台机器上
 * 有哪些钥匙），只是把"用这把钥匙去那一家问一次"这一步单独摊开给页面 —— 没有新的东西要管，
 * 所以不该长成第二个口。
 */
const PROBE_PATH = '/home-config/probe'

/**
 * ★ **"这条钥匙能调哪些型号"**（2026-10-14 支级覆盖那一刀）。
 *
 * 与 [`PROBE_PATH`] 的关系：**问对方的是同一件事**（`GET <基址>/models`），区别只有一个 ——
 * **钥匙从哪儿来**。探测那一格按纪律**只从请求体来**（探测的语义就是"用**这一把**试一试"）；
 * 这一格反过来：**用清单里那一条已经存着的钥匙**（请求体里只有条目 `id`）。
 *
 * 为什么必须有它：页面手里**永远没有**那把钥匙（key 只到那台机器为止、永不回浏览器）⇒
 * 没有这一格，"这台机器上这把钥匙支持哪些型号"就只能靠人重贴一次钥匙才看得到。
 *
 * 两条纪律：**只读**（一个字节都不落盘）、**响应里没有 key**（形状与探测读数同一份）。
 */
const CATALOG_PATH = '/home-config/catalog'

/**
 * ★ **接上一支 Agent**（2026-10-15 远程接上那一刀）—— 这扇门上**唯一一条会改变执行端进程**的路。
 *
 * 为什么它与模型那几条**不同族**、也**不挂在 `/home-config` 底下**：
 * `/home-config*` 那几条管的都是"模型配置"（清单 / 绑定 / 试连 / 目录），它们是**读与写配置**；
 * 而这一条要**在那台机器上建环境根、签凭据、起进程** —— 那是**执行期控制**。架构红线那句
 * "执行期控制只经 Home 准入授权的独立控制路径或同域路由"说的就是它：走这台机器自己的准入
 * （家钥匙 ＋ TLS），**不成为世界的耐久公共事实**。所以它自成一格路径，而不是塞进配置那几条。
 *
 * 四条边界（主人逐条裁过；实现全在 `remote-control.js`，这里只管形状与分流）：
 * 1. **准入 ≠ 授权**：钥匙回答"能不能进这台机器"，"允许我替你起进程"由 `home.conf` 的
 *    `remote_control=on` **单独**授权，**只能在本地打开**；没开 ⇒ 403 `remote_control_off`；
 * 2. **TLS / 来源**：与这扇门别的路**同一条**判据（模式二：钥匙 ＋ TLS；模式一：来源），不新增、不叠加；
 * 3. **谁付钱**：这一步**永不选模型**（没模型 ⇒ 接上但先不起它）；
 * 4. **只回读数**：结局从**那台机器自己的账本**读出来（`homed` / `homed-not-started` /
 *    `already-homed` / `failed`），原样读数在 `lines` 里。
 *
 * ⚠ 每次调用都在那台机器上留一行痕（`logs/remote-control.log`）。
 */
const CONTROL_PATH = '/home-control/attach'

/**
 * **来路**（只用于留痕）：浏览器的 `Origin` 头，或那个连接的对端地址。
 *
 * ⚠ 它**不参与任何判断** —— 判断只有那两道闸（来源 / 家钥匙）。这一格存在的唯一理由：
 * "有人在你这台机器上接了一支 Agent"这件事必须能被追溯（`logs/remote-control.log`）。
 * **不含钥匙、不含任何凭据**（与家钥匙那五条纪律里的"原文不进日志"同一条）。
 */
function sourceOf(req) {
  const origin = req.headers?.origin
  if (typeof origin === 'string' && origin.trim() !== '') return origin.trim()
  const remote = req.socket?.remoteAddress
  return typeof remote === 'string' && remote !== '' ? remote : null
}

/** 请求体上限：这是一份四件套，不是上传通道。 */
const BODY_LIMIT_BYTES = 64 * 1024

/** 读**试连回执**的上限：模型目录可能很长，但它是"顺手读一格"的读数，不是要落盘的数据。 */
const MODEL_BODY_LIMIT_BYTES = 512 * 1024

/** 试连窗口：够慢的网也该有话说；超时就如实说"没连上"。 */
const PROBE_TIMEOUT_MS = 8000

/**
 * 稳定错误词表（**封闭表**：client 的解码端配同一份，未知取值明确拒绝）。
 *
 * 同一刀在 `client/src/api/homeConfig.ts` 里也有一份——两侧逐字对齐。
 */
const ERRORS = {
  NOT_FOUND: 'not_found',
  METHOD_NOT_ALLOWED: 'method_not_allowed',
  ORIGIN_NOT_ALLOWED: 'origin_not_allowed',
  BAD_REQUEST: 'bad_request',
  BODY_TOO_LARGE: 'body_too_large',
  INVALID_ENDPOINT: 'invalid_endpoint',
  INVALID_MODEL: 'invalid_model',
  INVALID_WINDOW: 'invalid_window',
  INVALID_PROTOCOL: 'invalid_protocol',
  /**
   * **这一份模型收不收得到媒体材料**那一格不是布尔（2026-10-17）。
   *
   * ⚠ 它是**三态**的：**没说**（`undefined`）与**说了不支持**（`false`）是两件事——
   * 前者由 `upsertEntry` 从旧条目**原样带过来**（老页面不认识这一格，不许因此删掉它），
   * 后者是用户的明确决定。**形状不对一律拒绝，不静默当成"没说"**（那会把一次笔误
   * 读成"用户没打算说"，而它其实说了——只是说错了）。
   */
  INVALID_VISION: 'invalid_vision',
  /** 清单条目的 `id` 形状不对（`ms-<序号>`）。 */
  INVALID_ID: 'invalid_id',
  /** 条目的 `name` 不是字符串。 */
  INVALID_NAME: 'invalid_name',
  /** 那个"存完要不要绑定它"的开关不是布尔。 */
  INVALID_BIND: 'invalid_bind',
  /** 窗口留空、而试连也没能问出来（连不上 / 接口没给那一格）⇒ 这时才需要用户手填。 */
  WINDOW_UNAVAILABLE: 'window_unavailable',
  /**
   * ★ **钥匙没被对方认**（`/models` 回了 401 / 403）—— 与 `WINDOW_UNAVAILABLE` **刻意分开**。
   *
   * 为什么必须分：这两种病因**要用户做的事正好相反** ——
   * 前者该去**换钥匙**（此时手填窗口也没用：钥匙不对，填了照样连不上）；
   * 后者才该**手填窗口**（换钥匙没用：人家就是不在目录里给这一格）。
   *
   * 合并成一个码、共说一句"没能自动问出上下文长度"，等于**把用户指向错的那件事**
   * —— 实测踩过：用户看到 401 去查钥匙，就算换对了，这一格照样拿不到，于是他以为是别的问题。
   */
  CREDENTIAL_REJECTED: 'credential_rejected',
  /** 要更新/绑定/删除的那条清单条目不在场（悬空绑定也走这一格）。 */
  ENTRY_NOT_FOUND: 'entry_not_found',
  /** 那条清单条目**正被这个 Agent 绑定** ⇒ 不许删（先把绑定换走）。 */
  ENTRY_BOUND: 'entry_bound',
  /**
   * ★ **这台机器上一支住户都没有**（2026-10-12）：换绑 / 解绑问的是"哪一支用哪一条"，
   * 而住户表是空的 ⇒ **这件事没有目标**。不许静默成功（从前那条兜底会合成一支老家形状的
   * 住户，于是这一笔写下一份不属于任何人的绑定，快照还报"已指定"）。
   */
  NO_RESIDENT: 'no_resident',
  /**
   * ★ **这台机器上没有这一支**（2026-10-12 住户级覆盖）：`PUT /home-binding` 带了 `agentId`，
   * 而住户表里没有它（多半是刚被搬走/删掉，页面手里的那份读数旧了）⇒ 明确拒绝，
   * **绝不许**退回"写每一支"（那会把一次点错变成一次全体改动）。
   *
   * ⚠ **2026-10-14「选择先于接上」之后，本门不再产生这个码**：点了名的 `agentId` 只要
   * **形状合法**，就落在她将来会住的那个环境根上（见 `writeTargetsOf`）。
   * 它照旧留在封闭表里，因为**旧版的门**（远程那台还跑着老版本）还这么答 —— 那一侧
   * 客户端要说得清"先接上再选"，而不是把一个不认识的码摆给用户看。
   */
  NO_SUCH_RESIDENT: 'no_such_resident',
  /**
   * ★ **`agentId` 的形状不合法**（2026-10-14）：它会变成一个**目录名**
   * （`<家>/agents/<id>/`）⇒ 空 / 带空白 / 超过 200 字符 / 含路径分隔符或冒号 / 是 `.` 或 `..`
   * 一律拒，**不拼路径**。与 `app/src-tauri/src/home_doors.rs` 的 `agent_id_shape_ok` 逐字同形。
   */
  BAD_AGENT_ID: 'bad_agent_id',
  /**
   * ★ **思考档不在封闭表里**（2026-10-14 支级覆盖那一刀）：`reasoning_effort` 只有那 7 档
   * （权威是 `agent/src/model_source.rs` 的 `REASONING_EFFORTS`，本门镜像一份）——
   * 认不得的取值**在这一格就拒**，不许写进文件（写进去的值会让宿主拒绝启动）。
   */
  INVALID_EFFORT: 'invalid_effort',
  /**
   * ★ **这台机器没允许从外面接 Agent**（2026-10-15 远程接上那一刀）：家钥匙只回答"能不能进
   * 这台机器"，而"**允许我替你起进程**"是**另一件事** ⇒ 由 `home.conf` 的
   * `remote_control=on` 单独授权，且**只能在本地打开**（`node home.js remote-control on`）。
   * 缺省（没开）就是这一档 —— **fail-closed**，不是"装不存在"（404 会让人以为是版本不对）。
   */
  REMOTE_CONTROL_OFF: 'remote_control_off',
  /**
   * ★ **远程接上这一步没成**（2026-10-15）：那只手跑完了，而那一支**没被接上**
   * （环境根/住户登记还是不在）。**读数在 `lines` 里原样带出去** —— 它是"为什么没成"的唯一来路。
   */
  ATTACH_FAILED: 'attach_failed',
  KEY_REQUIRED: 'key_required',
  READ_FAILED: 'read_failed',
  SAVE_FAILED: 'save_failed',
  /**
   * ★ **家钥匙没带 / 不对**（家口远程访问刀；模式二专属）：没带钥匙、钥匙不对、摘要不在场
   * —— 对敲门的人都是同一件事（进不去），所以是**一个**码。
   *
   * ⚠ 它与 `KEY_REQUIRED` 不是一回事、也不许合并：那个说的是"保存一条模型源时的**模型** key"，
   * 这个说的是"进这个口的**家**钥匙"。两把钥匙归属不同（模型 key 归供应商，家钥匙归机器），
   * 要用户做的事也不同（去贴模型 key / 去那台机器重新拿家钥匙）。
   *
   * 码的权威在 `home-key.js`（产生它的那一层）——这里只把它收进封闭表。
   */
  UNAUTHORIZED: homeKey.UNAUTHORIZED,
}

/**
 * ⚠ 刀2b 把**私有的那两格**交给了 `model-sources.js`（清单 + 绑定的形状、原子写、迁移都在那儿）：
 * 本模块从前那份 `FILE_NAME` / `configFilePath` / `writeAtomic` / `readConfig`（单份配置文件那套
 * 机器）**已删** —— 免得出现"两份解析权威"，而那正是这一刀要治的东西。
 */

/**
 * 试连要探的地址：**模型目录**（`GET <基址>/models`，免费、无副作用）。
 *
 * ⚠️ **端点是完整地址**（Rust 侧的口径：`https://api.deepseek.com/chat/completions`，
 * 本层不替调用方拼路径）⇒ 直接 `endpoint + '/models'` 会探到
 * `<端点>/models` 这种不存在的路径。所以这里把**线协议那一段**摘掉再拼：
 * 最后一段是 `chat/completions` / `completions` / `responses` 时去掉它，剩下的就是基址；
 * 端点本来就是基址时原样用。
 *
 * 判据是可复核的（返回的 `url` 会如实回给页面），不是猜。
 */
function modelsUrl(endpoint) {
  const trimmed = endpoint.replace(/\/+$/, '')
  const stripped = trimmed.replace(/\/(chat\/)?completions$/, '').replace(/\/responses$/, '')
  return `${stripped}/models`
}

/**
 * 从 `/models` 的回执里读**标称窗口**时认哪些字段（**有序**：先命中的赢）。
 *
 * 为什么是一张表而不是"按名字随便找"：各家的模型目录形状不一样，我们**只读已经点名的字段**，
 * 并**如实回是哪一件命中的**（`windowField`）—— 探不到就探不到，不拿别的数字顶替。
 * 点号表示往下一层找（OpenRouter 那类把窗口挂在 `top_provider` 底下）。
 */
const WINDOW_FIELDS = [
  'context_length',
  'context_window',
  'max_context_length',
  'max_context_tokens',
  'context_size',
  'max_input_tokens',
  'input_token_limit',
  'max_model_len',
  'top_provider.context_length',
]

/** 只认正整数（或恰好是数字的字符串）；别的形状一律当"这一格没给"。 */
function positiveInteger(value) {
  if (typeof value === 'number' && Number.isInteger(value) && value > 0) return value
  if (typeof value === 'string' && WINDOW.test(value.trim())) {
    const parsed = Number.parseInt(value, 10)
    return parsed > 0 ? parsed : null
  }
  return null
}

/**
 * 从 `/models` 的回执里读出**目录条目数组**（三种形状：回执本身是数组 / `data` / `models`）。
 *
 * 只有这一处认识"目录长什么样"：读窗口（`windowFromModels`）与读模型名（`modelsFromBody`）
 * 都从它取 —— 免得两处各长一份形状判断。
 */
function catalogEntries(body) {
  if (Array.isArray(body)) return body
  if (body && typeof body === 'object' && Array.isArray(body.data)) return body.data
  if (body && typeof body === 'object' && Array.isArray(body.models)) return body.models
  return null
}

/**
 * 从 `/models` 的回执里读**模型名**（刀2b：把"模型目录"还给页面挑）。
 *
 * 三条边界，与窗口那一格同款（宁可少读也不猜）：
 * - 只认目录条目数组（见 `catalogEntries`）；
 * - 名字按 `id` / `name` / `model` 三选一取那个**非空字符串**，逐字原样（不截断、不改大小写）；
 * - **去重 + 封顶**（`MODEL_LIST_LIMIT`，保序）—— 它是"顺手读回来的目录"，不是要落盘的数据。
 *
 * 读不到 ⇒ `[]`（**不是错误**：目录没给这一格不影响"这家答不答、认不认这把钥匙"）。
 */
function modelsFromBody(body) {
  const entries = catalogEntries(body)
  if (!entries) return []
  const names = []
  for (const each of entries) {
    if (!each || typeof each !== 'object') continue
    const raw = [each.id, each.name, each.model].find(
      (value) => typeof value === 'string' && value.trim(),
    )
    if (!raw) continue
    const name = raw.trim()
    if (!names.includes(name)) names.push(name)
    if (names.length >= MODEL_LIST_LIMIT) break
  }
  return names
}

/** 按点号路径取一个值（中途不是对象就返回 undefined）。 */
function valueAt(entry, path) {
  return path
    .split('.')
    .reduce((node, key) => (node && typeof node === 'object' ? node[key] : undefined), entry)
}

const NO_WINDOW = { contextWindow: null, windowField: null }

/** 试连回执里最多带回多少条模型名（保序、去重后封顶）。 */
const MODEL_LIST_LIMIT = 200

// ---------------------------------------------------------------------------
// **原生端点**：有些家在自己的原生接口里给得比 OpenAI 兼容端点**更多**
// ---------------------------------------------------------------------------

/**
 * 为什么要有这一层：`/models`（OpenAI 兼容形状）**只保证给 id**，窗口是各家「自己加料」的事。
 * 实测 8 家「给 / 不给」各半（2026-09-23，窗口兜底表刀）——但那份「不给」**未必可靠**：
 * 至少 DeepSeek 是拿文档 schema 判的，当晚就被真响应推翻（2026-09-24 复测：`context_window`
 * 就在 `/v1/models` 响应里，`deepseek-flash` ⇒ 1048576）⇒ **「不给」必须以实测响应为准，
 * 接口多给字段不违约**。详见 `outputs/CLIENT_WINDOW_FALLBACK_KNIFE_2026-09-23.md` §八。
 * 而有些家的**原生**目录接口会把「分类 / 窗口 / 价格」一并给你 —— 阿里百炼就是：
 * `GET <base>/api/v1/models` 的每条带 `capabilities`（`["TG","VU","Reasoning"]`）与
 * `model_info.context_window`（实测 `qwen3.8-max` ⇒ `1000000`）。
 *
 * **表驱动：加一家 = 加一条**（不猜、不泛化）。表里没有的家 ⇒ 走原来的兼容端点，**行为一字不变**。
 *
 * ⚠ 每家要单独读，是因为**响应形状各家不同** —— 那是「外部事实」，不是我们能统一的。
 * 这也正是它**只对已知的家做**的原因：预设的家有限且已知；「自定义」不知道是哪一家，
 * 硬套一个原生端点就是猜。
 */
const NATIVE_PROBES = {
  /**
   * 阿里百炼（DashScope）—— **2026-09-24 实测**，目前表里唯一一家。
   *
   * - 路径 `GET <base>/api/v1/models`（与兼容端点 `/compatible-mode/v1/models` 是**两个目录**：
   *   原生 **517** 条、兼容 **261** 条）；
   * - 鉴权 `Authorization: Bearer <key>`（试过 `X-DashScope-API-Key` ⇒ 它回 "No API-key provided"，不认）；
   * - 分页 `page_no` / `page_size`，**`page_size` 上限 100**（500 回 `code: 10110400`）⇒ 517 条 **6 页**；
   * - 条目字段：`model` / `name` / `description` / `features` / `prices` / `provider` /
   *   **`capabilities`** / `published_time` / `inference_metadata` / **`model_info`** / …
   */
  dashscope: { path: '/api/v1/models', pageSize: 100 },
}

/**
 * 原生目录的候选封顶（兼容端点那份是 [`MODEL_LIST_LIMIT`]）。
 * 实测百炼原生 517 条 ⇒ 600 留余量；它只在「探到了」时才回来，读不到照旧走兼容端点。
 */
const NATIVE_LIST_LIMIT = 600

/** 端点 URL 的 origin（`https://api.x.com`）；读不懂 ⇒ `null`（调用方退回兼容端点）。 */
function originOf(endpoint) {
  try {
    return new URL(String(endpoint).trim()).origin
  } catch {
    return null
  }
}

/**
 * 百炼 `capabilities` 那些**短码**是什么意思（它文档里的原文；**认不出的一律不猜**）。
 *
 * ★ 注意 `VU`（视觉理解）与 `Reasoning` 都归 **`chat`** —— 它们**能对话**。
 * 只有 `VG`（video generation）是真·别的东西。
 */
const DASHSCOPE_CAPABILITY_KINDS = {
  TG: 'chat', // text generation
  VU: 'chat', // vision understanding —— 能对话
  Reasoning: 'chat', // 推理模型也是对话模型
  'Multimodal-Omni': 'chat',
  'Realtime-Omni': 'chat',
  VG: 'video', // video generation
}

/**
 * 从百炼条目的 `capabilities` 推出我们那套类别。
 *
 * ★ **保守**：认不出的码**不产出**（宁可不标，也不给一个错的标签）——
 * 给不出时，页面那边会按**名字**判（`modelCatalog.ts` 的「判不出来当 chat」）。
 */
function kindsFromDashscope(capabilities) {
  if (!Array.isArray(capabilities)) return null
  const kinds = new Set()
  for (const cap of capabilities) {
    const kind = DASHSCOPE_CAPABILITY_KINDS[String(cap)]
    if (kind) kinds.add(kind)
  }
  return kinds.size ? [...kinds] : null
}

/**
 * 探一个家的**原生目录**；表里没有 / 探不动 / 形状认不出 ⇒ **返回 `null`**（调用方退回兼容端点）。
 *
 * ★ 这里**绝不抛**：原生端点是"锦上添花"，它坏了不该让整个试连失败 —— 跟「兜底表读不动
 * 就当空表」同一条纪律。
 *
 * 返回 `{ provider, url, status, ok, models, modelMeta, total }`，其中
 * `modelMeta[id] = { kinds?, window? }` —— **kinds 是分类**（页面拿去打标签），
 * **window 是对方自己说的窗口**（比我们那张社区兜底表**新**）。
 */
async function probeNative(provider, baseUrl, credential, options = {}) {
  const spec = NATIVE_PROBES[provider]
  if (!spec || !baseUrl || !credential) return null
  const fetchImpl = options.fetch ?? globalThis.fetch
  const timeoutMs = options.nativeTimeoutMs ?? PROBE_TIMEOUT_MS

  const models = []
  const modelMeta = {}
  let total = null
  let page = 1
  // 翻页拉全（百炼 517 条 / 每页 100 ⇒ 6 页）。**上限兜底**：万一上游把 total 报错，不许无限翻。
  const MAX_PAGES = 12
  while (page <= MAX_PAGES) {
    const url = `${baseUrl}${spec.path}?page_no=${page}&page_size=${spec.pageSize}`
    let body
    try {
      const res = await fetchImpl(url, {
        method: 'GET',
        headers: { authorization: `Bearer ${credential}`, accept: 'application/json' },
        signal: AbortSignal.timeout(timeoutMs),
        cache: 'no-store',
      })
      if (!res.ok) return null
      body = await res.json()
    } catch {
      return null
    }
    const rows = body && body.output && Array.isArray(body.output.models) ? body.output.models : null
    if (!rows) return null
    for (const row of rows) {
      const id = row && typeof row.model === 'string' ? row.model.trim() : ''
      if (!id) continue
      if (!models.includes(id)) models.push(id)
      const meta = {}
      const kinds = kindsFromDashscope(row.capabilities)
      if (kinds) meta.kinds = kinds
      const win = row.model_info ? row.model_info.context_window : null
      if (typeof win === 'number' && Number.isInteger(win) && win > 0) meta.window = win
      if (Object.keys(meta).length > 0) modelMeta[id] = meta
    }
    const reported = body && body.output ? body.output.total : null
    if (typeof reported === 'number') total = reported
    if (rows.length < spec.pageSize) break
    if (total !== null && models.length >= total) break
    page += 1
  }
  if (models.length === 0) return null
  return { provider, url: `${baseUrl}${spec.path}`, status: 200, ok: true, models, modelMeta, total }
}

/**
 * 从 `/models` 的回执里读出**我们声明的那个模型**的标称窗口（TASK_77 回炉③）。
 *
 * 三条边界，都是"宁可少读也不猜"：
 * - 只在**目录条目数组**里找（`data` / `models`，或回执本身是数组）——别的位置不认识；
 * - 按 **id 精确命中**我们声明的那个模型名（`id` / `name` / `model` 三选一，与原值逐字比）；
 *   命不中 ⇒ 没读到（**不回落"取第一条"**，那是猜）；
 * - 命中的条目里按 `WINDOW_FIELDS` 顺序找第一个正整数 ⇒ 连**字段名**一起如实交出来。
 *
 * ⚠ 它只回**窗口那两格**：目录那份是 `modelsFromBody` 的事（两个读数各自独立，别混成一个）。
 */
function windowFromModels(body, model) {
  const entries = catalogEntries(body)
  if (!entries || !model) return { ...NO_WINDOW }
  const entry = entries.find((each) => {
    if (!each || typeof each !== 'object') return false
    return each.id === model || each.name === model || each.model === model
  })
  if (!entry) return { ...NO_WINDOW }
  for (const field of WINDOW_FIELDS) {
    const value = positiveInteger(valueAt(entry, field))
    if (value !== null) return { contextWindow: value, windowField: field }
  }
  return { ...NO_WINDOW }
}

/** 读试连回执的体（**有上限**）；读不动 / 不是 JSON / 太大 ⇒ `null`（一律当"没读到"）。 */
async function readModelsBody(response) {
  try {
    const text = await response.text()
    if (text.length > MODEL_BODY_LIMIT_BYTES) return null
    return JSON.parse(text)
  } catch {
    return null
  }
}

/**
 * 试连一次，**顺手把窗口与模型目录带回来**。**看的是"那家答不答、认不认这把钥匙"**，
 * 不是"模型能不能跑"。
 *
 * 三种读数分开（与全仓其它"读不到"的处置同款）：
 * - 拿到了 HTTP 应答 ⇒ `status` 是那个码，`reason` 为 null（`ok` = 2xx）；
 * - 连不上 / 超时 / 地址读不懂 ⇒ `status` 为 null，`reason` 说明是哪一种；
 * - 窗口与模型名是**另外两格读数**：只有 2xx 的回执才去看（错误页里的字段不是供应商的事实），
 *   读到就报值（窗口连**字段名**一起报），读不到就是 null / 空数组 —— **它们不改变试连本身的结论**。
 *
 * ⚠ 体只读一次：窗口与目录从同一份里取（两次 `text()` 在流上会读到空）。
 */
async function probeModels(endpoint, credential, model = '', options = {}) {
  const url = modelsUrl(endpoint)
  const fetchImpl = options.fetch ?? globalThis.fetch
  const timeoutMs = options.timeoutMs ?? PROBE_TIMEOUT_MS
  let response
  try {
    response = await fetchImpl(url, {
      method: 'GET',
      headers: { authorization: `Bearer ${credential}`, accept: 'application/json' },
      signal: AbortSignal.timeout(timeoutMs),
      cache: 'no-store',
    })
  } catch (error) {
    const name = error && error.name ? error.name : ''
    // ⚠ **`models` 这一格每一档都要在**（`{ ...NO_WINDOW, models: [] }`）：它是"这家有哪些型号"
    // 的读数，而**缺场**与"空数组"在客户端是两件事 —— 缺了它，整份读数会被判成"认不出的形状"
    // ⇒ 页面只能把"连不上"说成"读不到这份读数"，而按纪律它该说"没能问到这家"。
    // 壳那一侧（`home_doors.rs` 的 `no_window`）**一直带着它** ⇒ 这里从前是两门不同形
    // （2026-10-14 真机撞上：选型号那一格点了没反应）。
    return {
      url,
      ok: false,
      status: null,
      reason: name === 'TimeoutError' || name === 'AbortError' ? 'timeout' : 'unreachable',
      ...NO_WINDOW,
      models: [],
    }
  }
  const ok = response.status >= 200 && response.status < 300
  let catalog = { ...NO_WINDOW, models: [] }
  if (ok) {
    const body = await readModelsBody(response)
    if (body !== null) {
      catalog = { ...windowFromModels(body, model), models: modelsFromBody(body) }
    }
  }
  return {
    url,
    ok,
    status: response.status,
    reason: null,
    ...catalog,
  }
}

/**
 * 来源校验（第二道，落在 **Host 与 Origin 两个头**上）—— **模式一专属**，实现只有一处
 * （`home-key.js` 的 `localOriginAllowed`，两口共用；本文件不再各留一份）。
 *
 * ⚠ **模式二不调用它**（2026-09-25 修）：mode 二里浏览器**本来就是跨源的** ——
 * 远程条目的地址必然是 `https://<家地址>:<config_port>`，`Host` 因此**必然非 loopback**。
 * 在这里无条件卡一道，等于把「跨机改模型配置」整条路堵死（实测：带**有效家钥匙**
 * 直连远程门 ⇒ 403 origin_not_allowed，页面只能显示"只允许从本机访问"）。
 * 模式二的闸是**家钥匙 + TLS**（`authorize` / `resolveLaunch` 两条 fail-closed），
 * 而 mode 二的预检与 CORS 头正是为这个跨源浏览器而设（`preflight` / `attachCors`）。
 */
function originAllowed(req, access) {
  if (access && access.mode === 'remote') return true
  return homeKey.localOriginAllowed(req)
}

function respond(res, status, payload, extraHeaders = {}) {
  res.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    ...extraHeaders,
  })
  res.end(JSON.stringify(payload))
}

/** 读一个带上限的请求体（超限就断开：不为了收完一个 64 KiB 以上的体而先分配）。 */
function readBody(req, limit = BODY_LIMIT_BYTES) {
  return new Promise((resolve) => {
    const chunks = []
    let size = 0
    req.on('data', (chunk) => {
      size += chunk.length
      if (size > limit) {
        resolve({ ok: false, error: ERRORS.BODY_TOO_LARGE })
        req.destroy()
        return
      }
      chunks.push(chunk)
    })
    req.on('end', () => resolve({ ok: true, text: Buffer.concat(chunks).toString('utf8') }))
    req.on('error', () => resolve({ ok: false, error: ERRORS.BAD_REQUEST }))
  })
}

/**
 * 校验 PUT/POST 交上来的那一条（四件套 + 可选协议 + 可选 `name` / `bind`）。
 * 返回 `{ ok: true, declaration }` 或 `{ ok: false, error }`。
 *
 * **`id` 不在这里读**——"这一条存的是哪一格"归 [`pickEntryId`] 一处判（**一个权威**），
 * 这里只管**形状**。因此本函数要的"钥匙留空时沿用哪一把"由调用方算好交进来
 * （`fallbackKey`：更新哪一条就沿用哪一条的；新建没有可沿用的东西 ⇒ null）。
 *
 * **窗口可留空**（TASK_77 回炉③）：窗口**不是配置，是体检** —— 留空 ⇒ 试连时顺手向服务方问
 * （见下面那一支）。给了就必须是正整数（`0` 不是一条可解释的声明）；给了**垃圾**是
 * `invalid_window`，与"留空"是两件事（不把"填错了"混成"没填"）。
 *
 * `bind`（可选布尔，**缺省 true** = 老页面那句"保存 = 用它"）：`false` 表示**只存清单、
 * 不动绑定**——"清单归机器，绑定归 Agent"，清单页只管清单，"用哪一条"由模型房间说。
 */
function validateDeclaration(body, fallbackKey) {
  if (typeof body !== 'object' || body === null) return { ok: false, error: ERRORS.BAD_REQUEST }
  const endpoint = typeof body.endpoint === 'string' ? body.endpoint.trim() : ''
  if (!endpoint) return { ok: false, error: ERRORS.INVALID_ENDPOINT }
  let parsed
  try {
    parsed = new URL(endpoint)
  } catch {
    return { ok: false, error: ERRORS.INVALID_ENDPOINT }
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    return { ok: false, error: ERRORS.INVALID_ENDPOINT }
  }
  const model = typeof body.model === 'string' ? body.model.trim() : ''
  if (!model) return { ok: false, error: ERRORS.INVALID_MODEL }
  // 可选的 name（清单条目才有）：形状不对就**明确拒绝**，不猜。
  let name = null
  if (body.name !== undefined && body.name !== null && body.name !== '') {
    if (typeof body.name !== 'string') return { ok: false, error: ERRORS.INVALID_NAME }
    name = body.name.trim()
  }
  // 可选的"存完要不要绑"：形状不对 ⇒ 明确拒绝（不把 `"false"` 这种字符串当真）。
  let bind = true
  if (body.bind !== undefined && body.bind !== null) {
    if (typeof body.bind !== 'boolean') return { ok: false, error: ERRORS.INVALID_BIND }
    bind = body.bind
  }
  // 可选的**家**（预设 id，如 `deepseek`）：窗口留空、且探测也没读到窗口时，用它去兜底表里**限家**查一条。
  // 不传 / 传 `custom` / 传不认识的值 ⇒ **一律不查表**：自定义那条路不知道是哪一家，
  // 而"不知道是哪一家还去查"会**静默给错值**（实测：`mistral-large-latest` 被模糊命中成 32000，真值 256K）
  // ⇒ 宁可让它落到"用户自己填"。铁律见 `model-windows.js`。
  const provider = typeof body.provider === 'string' ? body.provider.trim() : ''
  const rawWindow = String(body.contextWindow ?? '').trim()
  let contextWindow = null
  if (rawWindow) {
    if (!WINDOW.test(rawWindow) || Number.parseInt(rawWindow, 10) <= 0) {
      return { ok: false, error: ERRORS.INVALID_WINDOW }
    }
    contextWindow = Number.parseInt(rawWindow, 10)
  }
  // 可选第五件：不给就用本家那一条（与 Rust 侧同一口径）；给了就必须落在闭集里。
  let protocol = null
  if (body.protocol !== undefined && body.protocol !== null && body.protocol !== '') {
    if (typeof body.protocol !== 'string' || !PROTOCOLS.includes(body.protocol)) {
      return { ok: false, error: ERRORS.INVALID_PROTOCOL }
    }
    protocol = body.protocol
  }
  const given = typeof body.apiKey === 'string' ? body.apiKey : ''
  const credential = given.trim() ? given : (fallbackKey ?? '')
  if (!credential.trim()) return { ok: false, error: ERRORS.KEY_REQUIRED }
  // **这一份模型收不收得到媒体材料**（2026-10-17）：**三态**，不许压成两态——
  // 「**没说**」（`undefined`）与「**说了不支持**」（`false`）是两件事：前者交给
  // `upsertEntry` 从旧条目原样带过来（**老页面不认识这一格，不许因此把它删掉**），
  // 后者是用户的明确决定。⚠ 压成一格 = 那个保险当场失效，而失效的后果**没有任何报错**：
  // 用户以为配好了，而她静默地重新读不动图片。
  let vision
  if (body.vision !== undefined && body.vision !== null) {
    if (typeof body.vision !== 'boolean') return { ok: false, error: ERRORS.INVALID_VISION }
    vision = body.vision
  }
  return {
    ok: true,
    declaration: {
      endpoint,
      credential,
      model,
      contextWindow,
      protocol,
      name,
      bind,
      provider,
      vision,
    },
  }
}

/**
 * 校验**只读探测**交上来的那一份（`POST /home-config/probe`）。
 * 返回 `{ ok: true, probe: { endpoint, credential, model, protocol } }` 或 `{ ok: false, error }`。
 *
 * 与 [`validateDeclaration`] 的两处**不同**（都是有意的）：
 * - **不要求模型名**：探测问的是"这家答不答、认不认这把钥匙"，模型名还没挑（页面接着才挑）；
 * - **key 只从请求体来**：没有"留空沿用那一条"这回事 —— 探测的语义就是"用**这一把**试一试"。
 *   空 key ⇒ `key_required`（如实拒绝，不拿别处的一把顶上来）。
 *
 * 复用同一份**闭集与同语法预检**（`PROTOCOLS` / URL 形状），不另写一套形状判断。
 */
function validateProbe(body) {
  if (typeof body !== 'object' || body === null) return { ok: false, error: ERRORS.BAD_REQUEST }
  const endpoint = typeof body.endpoint === 'string' ? body.endpoint.trim() : ''
  if (!endpoint) return { ok: false, error: ERRORS.INVALID_ENDPOINT }
  try {
    const parsed = new URL(endpoint)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return { ok: false, error: ERRORS.INVALID_ENDPOINT }
    }
  } catch {
    return { ok: false, error: ERRORS.INVALID_ENDPOINT }
  }
  // 线协议：给了就必须落在闭集里（**只做形状预检**：探测打的是 `/models`，与线协议无关）。
  if (body.protocol !== undefined && body.protocol !== null && body.protocol !== '') {
    if (typeof body.protocol !== 'string' || !PROTOCOLS.includes(body.protocol)) {
      return { ok: false, error: ERRORS.INVALID_PROTOCOL }
    }
  }
  // 可选的模型名：给了就顺手把**它**的标称窗口读回来（复用同一个试连）；不给就是纯拉目录。
  const model = typeof body.model === 'string' ? body.model.trim() : ''
  const credential = typeof body.api_key === 'string' ? body.api_key.trim() : ''
  if (!credential) return { ok: false, error: ERRORS.KEY_REQUIRED }
  // 可选的**家**（预设 id）：原生目录层的查找键 —— 它只是个键，查不中就退兼容端点，
  // 所以**不做闭集校验**（不是协议词表；「自定义」那格传什么都不该 4xx）。
  const provider = typeof body.provider === 'string' ? body.provider.trim() : ''
  return { ok: true, probe: { endpoint, credential, model, provider } }
}

/**
 * 这一条存的是**哪一格**（刀2b）：`body.id` **只在这一处被读**。
 *
 * - **`POST`（新建）**：id 由**门**给（`nextId`，读清单的 `next` 计数器 ⇒ **单调、永不回收**）；
 *   调用方指定 id 一律拒绝 ——那是新建，不是覆盖。
 * - **`PUT` 给了 id**：那一条必须**在场**（不在场 = `entry_not_found`，绝不悄悄新建一条）。
 * - **`PUT` 没给 id**：**当前绑定的那一条**（老页面那个"保存"就是它）；连绑定都没有 ⇒ 新建。
 *
 * 返回 `{ ok: true, id, previous, creating }` 或 `{ ok: false, error }`。
 */
function pickEntryId(body, { list, bound, create }) {
  const entries = list.entries
  const raw = typeof body.id === 'string' ? body.id.trim() : ''
  if (create) {
    if (raw) return { ok: false, error: ERRORS.BAD_REQUEST }
    return { ok: true, id: modelSources.nextId(list), previous: null, creating: true }
  }
  if (raw && !modelSources.ID_SHAPE.test(raw)) return { ok: false, error: ERRORS.INVALID_ID }
  const previous = raw ? (entries.find((entry) => entry.id === raw) ?? null) : null
  if (raw && previous === null) return { ok: false, error: ERRORS.ENTRY_NOT_FOUND }
  const id = raw || bound || modelSources.nextId(list)
  const target = previous ?? entries.find((entry) => entry.id === id) ?? null
  return { ok: true, id, previous: target, creating: target === null }
}

/**
 * 起模型门。返回 `{ server, port }`（调用方负责 log 与生命周期）。
 *
 * **两个文件、各管各的**（刀2b：清单归机器，绑定归 Agent）：
 * `<家>/model_sources.json` 与 `<环境根>/model_binding.json` —— 形状与迁移的唯一权威
 * 在 `model-sources.js`，本模块只做门面。
 *
 * @param {object} ctx `{ homeDir, envDir, probe }`（`probe` 是注入点：用例给它一个替身，
 *   把"保存后到底试连了几次、URI 是什么"变成可判据）。替身的签名与 `probeModels` 一致：
 *   `probe(endpoint, credential, model)`；返回的形状见 `probeModels`（含窗口那两格）。
 * @param {{now?: () => Date, access?: {mode: 'local'} | {mode: 'remote', homeDir: string}, tls?: {cert: Buffer, key: Buffer} | null}} [options]
 *   用例注入的时钟；**准入**（缺省 `{mode:'local'}` = 与从前一字不差：无钥匙、无 CORS、
 *   无预检）；`tls` 给了就用 `https` 起监听（模式二由 `serve()` 装配）。
 */
function createConfigServer(ctx, options = {}) {
  // 兜底表（探测拿不到窗口时查）。数据是静态的 ⇒ 默认读一次；**测试可注入**（`ctx.windowTable`，
  // 与 `ctx.probe` 同一条约定）。
  const windowTable = ctx.windowTable ?? loadWindowTable()
  const now = options.now ?? (() => new Date())
  /**
   * ★ **接上那一支的那只手**（2026-10-15）：缺省就是 `remote-control.js` 里那一份真实现
   * （它跑既有的操作路径 `node home.js up --agent <id> --attach-only`）。
   *
   * 可注入（`ctx.attach`，与 `ctx.probe` 同一条约定）：用例靠它把"这条路由到底把什么交出去"
   * 变成可判据，而门里只该有**一处**出口 —— 谁也不许在这里另写一只接上的手。
   */
  const attach = ctx.attach ?? remoteControl.attachRemote
  // **准入**：缺省模式一（和从前一字不差）。模式二由 `serve()` 按绑定地址装配。
  const access = options.access ?? { mode: 'local' }
  const tls = options.tls ?? null
  const homeDir = ctx.homeDir
  // **形状 fail-closed**：`residents` 只收**函数**（每次现问）。传一个数组进来会在下面被静默
  // 当成"没有住户表"（⇒ 退回老家那一支的合成读数）—— 而"拿一份**会过期的快照**当住户表"正是
  // 2026-10-12 治的那个病（门把开口那一刻的世界当成现在）⇒ 爆在开口那一刻，别让它悄悄读错。
  if (ctx.residents !== undefined && typeof ctx.residents !== 'function') {
    throw new Error('ctx.residents 必须是函数（每次现问的住户表）；数组是一种会过期的快照')
  }
  // —— **一个家住 N 支**（2026-10-01 多支刀）：绑定落在**每一支**的环境根上 ——
  //
  // 这个门读写的是「清单 + 绑定」两份文件，而**绑定归 Agent**（`<环境根>/model_binding.json`）。
  // 一个家住几支就有几个环境根 ⇒ 这里的每一笔写都落在**所有住户**的环境根上（"这台机器上这个家
  // 用哪条模型"是**家级选择**，一支一份只是它的落盘形状），读则把各支并起来如实报：
  // 都绑同一条 ⇒ 就是那一条；**各支不一致 ⇒ 如实标出来**（`bindingDivergent`），不替它们选边。
  //
  // 旧调用点（用例注入 `ctx.envDir`）⇒ 只有一支，与从前一字不差。
  //
  // ★ **2026-10-12：住户表每次现问，不在开口那一刻冻一份** —— 家是活的：门起来之后还能安家
  //   （`node home.js up --agent`）、也能搬走一支。冻一份表的两个症状**都是"把边界说成事实"**：
  //   门起来之后才安的那一支 ⇒ 逐支读数里**没有它**；已经搬走/删掉的那一支 ⇒ **还占着一行**。
  //   ⚠ 这与 `agent/src/model_source.rs` 的"本模块不缓存、不派生第三份"是**同一条纪律**：
  //   **权威永远是那两份文件，门只是通道**。
  /** 这一份读数里的住户（一支一份 `model_binding.json`；还没住户化的老家那一支也在里面）。 */
  const residentEnvsOf = () => {
    // ⚠ **`() => []` 是一个真答案**（这个家还没有住户），不是"没给" —— 不许退回老家那一支：
    //   那会**合成**一支不存在的住户，逐支读数里多出一行幽灵，写还会落到一份不属于任何人的文件上。
    if (typeof ctx.residents === 'function') {
      return ctx.residents().map((resident) => ({ agentId: resident.agentId ?? null, envDir: resident.envDir }))
    }
    // 旧调用点（用例只给 `agentId` / `envDir`，没有住户表）⇒ 合成一支，与从前一字不差。
    return [{ agentId: ctx.agentId ?? null, envDir: ctx.envDir ?? P.legacyEnvRoot(homeDir) }]
  }

  /** 所有住户的环境根 —— **写**也要落在**每一支**上（现问，理由同上一条）。 */
  const envDirsOf = () => residentEnvsOf().map((resident) => resident.envDir)

  /**
   * **这一笔写落在哪几支上**（2026-10-12 住户级覆盖；2026-10-14「选择先于接上」扩了一格）。
   *
   * - `agentId` **没给** ⇒ 这台机器上的**每一支**（家级默认：一个家住 N 支就写 N 份）；
   * - `agentId` **给了** ⇒ **只写那一支**（"住户用哪把钥匙归住户"：一支一份绑定本来就是住户级事实，
   *   缺的只是"只给某一支换"这条路）；
   * - `agentId` **给了、而这台机器上还没有这一支** ⇒ 写**她将来会住的那个环境根**
   *   （`<家>/agents/<id>/`）：**选择先于接上** —— 绑定的落点本来就是"她的环境根"，
   *   而环境根要等接她的第 2 步才建。⚠ 这一格曾经是拒绝（`no_such_resident`），于是
   *   "先给她选好、再接上"这条路在页面上走不通（2026-10-14 主人撞上：机器上有两条来源，
   *   而接她的那一步按"选哪一条 = 花谁的钱，不替她决定"停在原地，两边互等）。
   *
   *   ⚠ **形状不合法不许走到这里**（调用方先过 `agentIdShapeOk`，那一格回 `bad_agent_id`）：
   *   这一格的值会变成一个目录名，不预检就是目录穿越。
   *   ⚠ 空目录**不算住户**（`residents()` 要 `agent.conf` 或 `identity.json`）⇒ 写下的这一笔
   *   不会凭空造出一支住户来（写失败时留下的空目录同理，与壳侧 `bind_first_entry_if_unbound` 同款）。
   */
  const writeTargetsOf = (agentId) => {
    const residents = residentEnvsOf()
    if (agentId === null) return residents.map((resident) => resident.envDir)
    const mine = residents.filter((resident) => resident.agentId === agentId)
    if (mine.length > 0) return mine.map((resident) => resident.envDir)
    return [P.agentEnvRoot(homeDir, agentId)]
  }

  /**
   * **还没住户化、但已经先选好了**的那几支（2026-10-14）：单独一格，**不并进住户那几格**。
   *
   * 为什么必须单独一格：机器页那一列的主语是"这台机器上的住户"——把她们混进去，
   * 页面就会把一支还没接上机器的 Agent 画成住在这里（假话）。而她的页面要读得到
   * "我给她先选好的那条是哪条"（读不出来就会把"选过了"说成"没指定"，同样是假话）。
   */
  const unhomedRowsOf = () =>
    modelSources.unhomedBindings(homeDir, residentEnvsOf().map((resident) => resident.agentId))
  const probe = ctx.probe ?? probeModels
  // 原生目录探针（可注入，与 `ctx.probe` 同一条约定；缺省 = 真探）。
  const probeNativeFn = ctx.probeNative ?? probeNative

  /**
   * **探测的组合口**（原生目录层，2026-09-24）。
   *
   * 预设家若在 [`NATIVE_PROBES`] 表里有行 ⇒ 先探它的**原生目录**（那家的字段比兼容端点富：
   * 候选更多、带权威品类、有的还带窗口）；表里没有 / 原生探不动 / 形状认不出 ⇒ **一字不变**
   * 退回兼容端点（`probe`，可注入）。绝不抛：原生层坏了最多损失"加料"，不能砸了试连。
   *
   * - 窗口：原生给的也是"对方自己说的" ⇒ 来路照标 `probed`（已知家走原生时兼容端点根本不问，
   *   两份窗口不会并存；兜底表仍在更下游——"对方说的"永远赢过"社区说的"）；
   * - 候选：原生的列表（封顶 [`NATIVE_LIST_LIMIT`]）；`modelKinds` 是**按模型名**给的权威品类
   *   （页面拿去打标签，优先于按名字猜）——只有原生探成功才有这一格，空则整格不产出。
   */
  async function probeFull(endpoint, credential, model, provider) {
    const native = provider ? await probeNativeFn(provider, originOf(endpoint), credential) : null
    if (native !== null) {
      const meta = model ? (native.modelMeta[model] ?? null) : null
      const window =
        meta && typeof meta.window === 'number'
          ? { contextWindow: meta.window, windowField: 'model_info.context_window' }
          : { ...NO_WINDOW }
      const modelKinds = {}
      for (const [id, each] of Object.entries(native.modelMeta)) {
        if (Array.isArray(each.kinds) && each.kinds.length > 0) modelKinds[id] = each.kinds
      }
      return {
        url: native.url,
        ok: true,
        status: native.status,
        reason: null,
        ...window,
        models: native.models.slice(0, NATIVE_LIST_LIMIT),
        ...(Object.keys(modelKinds).length > 0 ? { modelKinds } : {}),
      }
    }
    return probe(endpoint, credential, model)
  }

  /**
   * **三合一**的快照（`GET /home-config` 的载荷）。
   *
   * 三层各说各的话：
   * - `sourcesState` / `entries` —— **清单**（机器级；key 一律只回 `keyConfigured`）；
   * - `bindingState` / `bound` —— **绑定**（这个 Agent 用哪一条）；
   * - `state` / `endpoint` / `model` / `contextWindow` / `protocol` / `keyConfigured`
   *   —— **由绑定那一条算出来的**那几个老字段。它是**投影**，不是第二份权威：
   *   唯一的权威永远是那两份文件（所以它每次现算，不缓存）。
   *
   * `state` 的三态沿用老口径（`configured` / `absent` / `unreadable`），但判据落在绑定上：
   * 绑了且解析得出 ⇒ configured；没绑 ⇒ absent；**绑定悬空**（指向已删条目）或清单读不出来
   * ⇒ unreadable（**不静默回落**成"没配"）。
   */
  const snapshot = () => {
    const list = modelSources.readList(homeDir)
    /**
     * ★ **见证**（2026-10-18）：这两份配置最后是**从这道门**改的吗？如果是，什么时候？
     *
     * 一次读全，然后**按份看**（`哪一份 ⇒ 它自己最后一条`）—— 不是"整本账里有没有人动过"。
     * 两档分开报：门改的**有时刻**；别处改的（她走 terminal、别的工具）**只知"变了"**。
     * 判据、边界与"不是封印"那条都在 `model_witness.js` 的文件头。
     */
    const witness = modelWitness.open(homeDir)
    const witnessOf = (file) => modelWitness.readingFor(witness, homeDir, file)
    // **住户现问**（2026-10-12）：门起来之后才安的那一支也在里面，搬走的那一支不在。
    const residentEnvs = residentEnvsOf()
    // **各支的绑定并起来**（一支一份文件）：都 absent ⇒ absent；有读不出来的 ⇒ unreadable；
    // 否则取**第一支绑的那条**作为家的答案，并把"各支是不是一致"如实标出来。
    const bindings = residentEnvs.map((resident) => ({
      agentId: resident.agentId,
      envDir: resident.envDir,
      binding: modelSources.readBinding(resident.envDir),
    }))
    const unreadableOne = bindings.find((each) => each.binding.state === 'unreadable')
    const boundOne = bindings.find((each) => each.binding.state === 'bound')
    const binding = unreadableOne
      ? { state: 'unreadable', source: null, reason: unreadableOne.binding.reason }
      : boundOne
        ? { state: 'bound', source: boundOne.binding.source }
        : { state: 'absent', source: null }
    const bindingDivergent =
      new Set(bindings.map((each) => (each.binding.state === 'bound' ? each.binding.source : null))).size > 1
    const bound =
      binding.state === 'bound' ? (list.entries.find((entry) => entry.id === binding.source) ?? null) : null
    const dangling = binding.state === 'bound' && bound === null
    const reason =
      list.state === 'unreadable'
        ? list.reason
        : binding.state === 'unreadable'
          ? binding.reason
          : dangling
            ? 'binding_dangling'
            : null
    const payload = {
      readAt: now().toISOString(),
      state:
        list.state === 'unreadable' || binding.state === 'unreadable' || dangling
          ? 'unreadable'
          : bound
            ? 'configured'
            : 'absent',
      reason,
      keyConfigured: bound ? bound.api_key.trim().length > 0 : false,
      endpoint: bound?.endpoint ?? null,
      model: bound?.model ?? null,
      contextWindow: bound?.context_window ?? null,
      protocol: bound?.protocol ?? null,
      sourcesState: list.state,
      entries: list.entries.map((entry) => ({
        id: entry.id,
        name: entry.name,
        endpoint: entry.endpoint,
        model: entry.model,
        contextWindow: entry.context_window,
        protocol: entry.protocol,
        keyConfigured: entry.api_key.trim().length > 0,
      })),
      bindingState: binding.state,
      bound: binding.state === 'bound' ? binding.source : null,
      // ★ **这台机器允不允许从别的机器接 Agent 上来**（2026-10-15 远程接上那一刀）。
      //   这一扇门只有两个值（`on` / `off`）—— 因为**它就是提供那条路的那一扇**；
      //   而"不提供"是**壳形态**那一侧的答案（它只绑 loopback，报 `null`）。
      //   三态各有各的意思，页面不许压成两态（见客户端 `remoteAttachReadout`）。
      remoteControl: remoteControl.remoteControlEnabled(homeDir) ? 'on' : 'off',
      // **各支的绑定**（本刀新增；多一个字段不管 —— 冻结的前端照旧读上面那几格）：
      // 一支一份，如实列出（`agentId` 读不到就是 null）。`bindingDivergent` = 各支绑得不一样。
      bindingDivergent,
      // ★ **这两份配置最后是谁改的**（2026-10-18；形状见 `model_witness.js`）：
      //   顶层这一格是**清单**那一份的见证；逐支那两格的每一行带着**它自己那份绑定**的见证
      //   （见证跟它见证的那一份走，不另开一张平行的表）。
      //   `witnessChainBreakAt`：账本链条的第一个断点（`null` = 完整）——
      //   "只追加 ＋ 链值"若没人读，它就是一个装饰。
      listWitness: witnessOf(modelSources.listPath(homeDir)),
      witnessChainBreakAt: witness.chain === null ? null : witness.chain.index,
      boundPerResident: bindings.map((each) => ({
        agentId: each.agentId,
        state: each.binding.state,
        bound: each.binding.state === 'bound' ? each.binding.source : null,
        // ★ **支级覆盖那两格**（2026-10-14）：这一支**具体用哪个型号 / 什么思考档**
        //   （`null` = 没覆盖，用它选的条目里的）。没有这两格，页面只能说"用哪一条来源"，
        //   而运行时**一直**认它们 ⇒ 那是"页面上说的"与"实际在用的"对不上。
        model: each.binding.state === 'bound' ? (each.binding.model ?? null) : null,
        reasoningEffort: each.binding.state === 'bound' ? (each.binding.reasoningEffort ?? null) : null,
        // ★ 这一支那份 `model_binding.json` 的见证（**可选**：老门不给 ⇒ 调用方如实说"读不到"）。
        witness: witnessOf(modelSources.bindingPath(each.envDir)),
      })),
      // ★ **还没住户化、但已经先选好了**的那几支（2026-10-14）：她还没被接到这台机器上，
      //   而有人已经在她的页面上给她选了哪一条 —— 单列一格，**不混进上面那几格**
      //   （混进去机器页就会把她画成住在这里）。老页面不认识这一格（多一个字段不管）。
      boundUnhomed: unhomedRowsOf().map((each) => ({
        agentId: each.agentId,
        state: each.binding.state,
        bound: each.binding.state === 'bound' ? each.binding.source : null,
        model: each.binding.state === 'bound' ? (each.binding.model ?? null) : null,
        reasoningEffort: each.binding.state === 'bound' ? (each.binding.reasoningEffort ?? null) : null,
        witness: witnessOf(modelSources.bindingPath(each.envDir)),
      })),
    }
    // `api_key` **一个字节都不在这个载荷里**：它只活在下面这份返回值的 `list` / `binding`
    // 里（请求处理期间的内存），从来没有输出通道。
    return { payload, list, binding }
  }

  /**
   * **一笔写落在给的那几支上**（缺省 = 所有住户；一支一份 `model_binding.json`，2026-10-01 多支刀）。
   * 任何一处失败都往上抛（调用方回 `SAVE_FAILED`）—— 半绑（只有一部分住户换了）不许静默。
   *
   * `overrides` 是**支级覆盖那两格**（`model` / `reasoningEffort`；三态见 `modelSources.writeBinding`），
   * 缺省 `{}` = 不动它们（旧行为一字不变）。
   */
  const writeBindingEverywhere = (source, dirs = envDirsOf(), overrides = {}) => {
    for (const dir of dirs) {
      modelSources.writeBinding(dir, source, overrides)
      // ⚠ **一支写成就记一条**（不是等整圈写完再记）：半途失败时，"真被改过的那几份"
      //    必须都有见证 —— 否则它们会读成"别处有人动过它"（把第一档报成第二档）。
      witnessWrite(modelSources.bindingPath(dir))
    }
  }
  /** 解除绑定：给的每一支都摘掉（缺省 = 每一支；撤销式：查无也成功）。**"清"也是一笔写**。 */
  const clearBindingEverywhere = (dirs = envDirsOf()) => {
    for (const dir of dirs) {
      modelSources.clearBinding(dir)
      witnessClear(modelSources.bindingPath(dir))
    }
  }
  /**
   * ★ **记一条见证**（2026-10-18）：这一笔写是**从这道门**出去的 ⇒ 记下"哪一份、什么时候、
   * 那一刻它的摘要"。两档（页面改的／别处改的）、不是封印、绝不拦写 —— 全在
   * `model_witness.js` 的文件头。
   *
   * ⚠ **缝在这道门上，不在 `modelSources.writeAtomic` 上**：那一个函数也承载**不是页面**的写
   * （`up.js` 的迁移、布置时的绑定携带）—— 在那里记账会把"布置"说成"页面改的"，
   * 而那正是这一层要防的那句假话。
   *
   * ⚠ **记不下绝不让这一笔写失败**：回一行 stderr，配置照旧是写成功的。
   */
  const witnessWrite = (file) => {
    const result = modelWitness.recordWrite(homeDir, file, now().getTime())
    if (!result.ok) console.error(`[见证] 这一笔没记下：${file}（${result.reason}）`)
  }
  /** **"清"也是一笔写**：不记它，页面上的"解除指定"就会读成"别处有人动过它"（第一档报成第二档）。 */
  const witnessClear = (file) => {
    const result = modelWitness.recordClear(homeDir, file, now().getTime())
    if (!result.ok) console.error(`[见证] 这一笔没记下：${file}（${result.reason}）`)
  }
  /** 有没有**任何一支**绑着这一条（删清单条目时的挡板：一支绑着就不许删）。 */
  const boundAnywhere = (id) =>
    envDirsOf().some((dir) => {
      const each = modelSources.readBinding(dir)
      return each.state === 'bound' && each.source === id
    })

  const handler = (req, res) => {
    // 家口远程访问刀：模式二下这三个动作按序发生（预检在认证**之前**——预检请求按规范
    // 不带 Authorization；模式一下三行全是空操作，行为与从前一字不差）。
    homeKey.attachCors(res, access)
    if (homeKey.preflight(req, res, access, ['GET', 'POST', 'PUT', 'DELETE'])) return
    if (!homeKey.authorize(req, res, access)) return
    const target = (req.url ?? '/').split('?')[0]
    // ★ **来源那道排在路由之前**（2026-10-11）：与壳那一侧**同形** ——
    //   `app/src-tauri/src/home_doors.rs` 是先判来源、再进路由。
    //   从前这里先把路由与方法判完才判来源 ⇒ 外域来源打 `/home-binding`（只收 PUT）
    //   拿到的是 **405 + `Allow: PUT`**（等于告诉一个不该知道的来源"这个端点存在、它认哪些动词"），
    //   而壳那边给的是 **403** ✗ —— 同一道门、两个生产者、两种答案，**取决于哪个生产者
    //   碰巧占着这两个口**（2026-10-11 真机门禁撞上：那条"外域来源 ⇒ 403 不是 405"读到 405，
    //   因为答话的是源码形态的门）。⚠ 合法来源什么也没少：来源过了之后，方法不对照样
    //   405 + `Allow`（下面那三条判据一个字没动）。
    if (!originAllowed(req, access)) {
      respond(res, 403, { error: ERRORS.ORIGIN_NOT_ALLOWED })
      return
    }
    // 端面是**穷举**的四条：`/home-config`（清单读写）、`/home-binding`（绑定读写）、
    // `/home-config/probe`（只读探测，钥匙从请求体来）与 `/home-config/catalog`
    // （只读目录，钥匙用清单里存着的那一条）。不"剥前缀随便走"，也不给自己留暗门。
    if (
      target !== '/home-config' &&
      target !== '/home-binding' &&
      target !== PROBE_PATH &&
      target !== CATALOG_PATH &&
      target !== CONTROL_PATH
    ) {
      respond(res, 404, { error: ERRORS.NOT_FOUND })
      return
    }
    // 方法先判：看家本领就四条（GET / POST / PUT / DELETE），`Allow` 如实列出来。
    if (!['GET', 'POST', 'PUT', 'DELETE'].includes(req.method)) {
      respond(res, 405, { error: ERRORS.METHOD_NOT_ALLOWED }, { allow: 'GET, POST, PUT, DELETE' })
      return
    }
    // `/home-binding` 只收 PUT（读绑定走在 `GET /home-config` 里，别处不开第二条读面）。
    if (target === '/home-binding' && req.method !== 'PUT') {
      respond(res, 405, { error: ERRORS.METHOD_NOT_ALLOWED }, { allow: 'PUT' })
      return
    }
    // 探测只收 POST：key 走请求体，**绝不进 URL**（URL 会进日志、进历史）。
    if (target === PROBE_PATH && req.method !== 'POST') {
      respond(res, 405, { error: ERRORS.METHOD_NOT_ALLOWED }, { allow: 'POST' })
      return
    }
    // 目录那一格也是 POST（请求体里是条目 id），同样**绝不进 URL**。
    if (target === CATALOG_PATH && req.method !== 'POST') {
      respond(res, 405, { error: ERRORS.METHOD_NOT_ALLOWED }, { allow: 'POST' })
      return
    }
    // 接上那一格也是 POST（请求体里是 Agent id）：**进 URL 就会进日志、进历史**。
    if (target === CONTROL_PATH && req.method !== 'POST') {
      respond(res, 405, { error: ERRORS.METHOD_NOT_ALLOWED }, { allow: 'POST' })
      return
    }

    if (req.method === 'GET') {
      respond(res, 200, snapshot().payload)
      return
    }

    if (req.method === 'DELETE') {
      const id = new URL(req.url ?? '/', 'http://127.0.0.1').searchParams.get('id')
      if (!id) {
        // **不带 id 的 DELETE = 解除绑定**（这是旧页面上"删除配置"那个动作的干净对应：
        // 它的文案本来就是"删除之后 Agent 走环境变量"）。撤销式 ⇒ 查无也成功、给幂等读数。
        try {
          clearBindingEverywhere()
        } catch {
          respond(res, 500, { error: ERRORS.SAVE_FAILED })
          return
        }
        respond(res, 200, { ...snapshot().payload, probe: null })
        return
      }
      // 带 id = 删清单条目。**被绑定挡住就拒绝**，并把"被谁绑着"如实说出来。
      // ⚠ 住户化之后要问**每一支**（一支绑着这条就不许删）。
      if (boundAnywhere(id)) {
        respond(res, 409, { error: ERRORS.ENTRY_BOUND, boundTo: id })
        return
      }
      // ⚠ 住户化之后要问**每一支**（一支绑着这条就不许删）—— 上面那道 `boundAnywhere` 是
      // 第一道挡板；模块层 `removeEntry` 自己也问**每一支**（2026-10-23：从前它只看第一支，
      // 而"第一支"在没有住户时会直接炸）。两处传的是**同一份住户表**（`envDirsOf()`）。
      const removed = modelSources.removeEntry(homeDir, envDirsOf(), id)
      if (!removed.ok) {
        if (removed.reason === 'bound') {
          respond(res, 409, { error: ERRORS.ENTRY_BOUND, boundTo: id })
          return
        }
        if (removed.reason === 'not_found') {
          respond(res, 404, { error: ERRORS.ENTRY_NOT_FOUND })
          return
        }
        respond(res, 500, { error: ERRORS.SAVE_FAILED })
        return
      }
      // 清单被这道门改过了 ⇒ 记一条（这一刻它的字节就是刚写下去的那一份）。
      witnessWrite(modelSources.listPath(homeDir))
      respond(res, 200, { ...snapshot().payload, probe: null })
      return
    }

    // ---- POST / PUT：两条路由 ----
    //
    // `/home-binding`：换绑 / 解绑（换绑 = **下一次活动**生效，沿用那句文案）。
    // `/home-config`：`POST` **新建一条**；`PUT` **更新一条**。两条都可能"顺便绑定它"
    // （`bind` 说，缺省 true —— 老页面那句"保存 = 用它"）。
    const creating = req.method === 'POST'
    void readBody(req).then(async (body) => {
      if (!body.ok) {
        respond(res, 400, { error: body.error })
        return
      }
      let parsedBody
      try {
        parsedBody = JSON.parse(body.text)
      } catch {
        respond(res, 400, { error: ERRORS.BAD_REQUEST })
        return
      }

      // ---- `/home-config/probe`：**只读探测**（探测即测试，保存才落盘）----
      //
      // 三条纪律落在这里：
      // - **不写任何文件**：这一支全程不碰 `modelSources`（清单与绑定一个字节都不动）——
      //   主人拍过的裁决，也是用例里那条"不落盘"判据守的东西；
      // - **响应里没有 key**：交回去的只有 `probeModels` 那份读数（url / ok / status / reason /
      //   窗口那两格 / 模型目录），它里面**没有**任何一格是请求里那把钥匙；
      // - **试连失败是读数不是错误**：连不上 / 超时照样 200，`probe.ok === false` 叫页面如实说
      //   "这家现在连不上"；只有**请求形状**不对（地址读不懂 / 没给钥匙 / 协议不在闭集）才 4xx。
      if (target === PROBE_PATH) {
        const checked = validateProbe(parsedBody)
        if (!checked.ok) {
          respond(res, 400, { error: checked.error })
          return
        }
        const result = await probeFull(
          checked.probe.endpoint,
          checked.probe.credential,
          checked.probe.model,
          checked.probe.provider,
        )
        respond(res, 200, { probe: result })
        return
      }

      // ---- `/home-config/catalog`：**用存着的那把钥匙**问一次"这条能调哪些型号"----
      //
      // 与上面那条的三条纪律**逐条相同**（只读 / 响应里没有 key / 连不上是读数不是错误），
      // 唯一的差别是**钥匙从哪儿来**：这里从清单里点名的那一条读（请求体只有 `id`）。
      // ⚠ `id` 认不出 ⇒ 404（与"改一条点名了不存在的 id"同一个码、同一个状态）。
      if (target === CATALOG_PATH) {
        const wantedId = typeof parsedBody.id === 'string' ? parsedBody.id.trim() : ''
        if (!wantedId) {
          respond(res, 400, { error: ERRORS.BAD_REQUEST })
          return
        }
        const list = modelSources.readList(homeDir)
        if (list.state === 'unreadable') {
          respond(res, 500, { error: ERRORS.READ_FAILED })
          return
        }
        const entry = list.entries.find((each) => each.id === wantedId)
        if (!entry) {
          respond(res, 404, { error: ERRORS.ENTRY_NOT_FOUND })
          return
        }
        // **兼容端点那条**（`GET <基址>/models`）：与探测那一格同一条路（原生目录层要"是哪一家"，
        // 而条目里没有那一格 ⇒ 不猜、不假装）。⚠ 走的是**可注入的那个探针**（`ctx.probe`），
        // 不是模块级的真实现 —— 用例靠注入判接线，门里只该有一处出口。
        const result = await probe(entry.endpoint, entry.api_key, entry.model)
        respond(res, 200, { probe: result })
        return
      }

      // ---- `/home-control/attach`：**接上一支 Agent**（2026-10-15 远程接上那一刀）----
      //
      // 这是这扇门上**唯一一条会改变执行端进程**的路（别的都在读写模型配置）。四条边界
      // （主人逐条裁过，实现全在 `remote-control.js`，这里只做形状与分流）：
      //
      // 1. **准入 ≠ 授权**：家钥匙回答"能不能进这台机器"，而"允许我替你起进程"由
      //    `home.conf` 的 `remote_control=on` **单独**授权，且**只能在本地打开** ——
      //    没开就是 `remote_control_off`（fail-closed，**不装成 404**）。
      // 2. **TLS / 来源**：与这扇门别的路**同一条**判据（模式二：钥匙 ＋ TLS；模式一：来源）——
      //    这一层不新增任何判据，也不叠加。
      // 3. **谁付钱**：这一步**永不选模型**（那只手只接上；没模型就接上但先不起它）。
      // 4. **只回读数**：结局（`homed` / `homed-not-started` / `already-homed` / `failed`）
      //    从**那台机器自己的账本**读出来，原样读数在 `lines` 里 —— 一个字的承诺都不给。
      //
      // ⚠ 每次都在那台机器上留一行痕（`logs/remote-control.log`）：谁、什么时候、从哪儿、接了哪一支。
      if (target === CONTROL_PATH) {
        const wantedId =
          typeof parsedBody.agentId === 'string' ? parsedBody.agentId.trim() : ''
        // 形状先过（它会变成一个目录名）——与 `/home-binding` 那一格**同一个判据、同一个码**。
        if (!wantedId || !agentIdShapeOk(wantedId)) {
          respond(res, 400, { error: ERRORS.BAD_AGENT_ID })
          return
        }
        // 那只手是**可注入的**（`ctx.attach`；缺省就是 `remote-control.js` 里那一份真实现）——
        // 用例靠它把"这条路由到底把什么交出去"变成可判据，而门里只该有一处出口。
        const result = await attach({ homeDir, agentId: wantedId, from: sourceOf(req) })
        if (!result.ok) {
          const status = result.error === ERRORS.REMOTE_CONTROL_OFF ? 403 : 500
          respond(res, status, { error: result.error, lines: result.lines ?? [] })
          return
        }
        respond(res, 200, {
          outcome: result.outcome,
          agentId: result.agentId,
          envRoot: result.envRoot,
          pid: result.pid,
          lines: result.lines ?? [],
        })
        return
      }

      if (target === '/home-binding') {
        // `agentId`（可选）：**只给这一支**换 / 解（住户级覆盖）；不给 = 这台机器上的每一支。
        const wanted =
          typeof parsedBody.agentId === 'string' && parsedBody.agentId.trim() !== ''
            ? parsedBody.agentId.trim()
            : null
        // ★ **点名的那一支先过形状**（2026-10-14）：它接下来会拼进路径
        //   （`<家>/agents/<id>/`）⇒ 形状不合法一律拒，不拼、不猜、不回落"写每一支"。
        if (wanted !== null && !agentIdShapeOk(wanted)) {
          respond(res, 400, { error: ERRORS.BAD_AGENT_ID })
          return
        }
        // ★ **一台机器上一支住户都没有 ⇒ 这件事没有目标**（"落在每一支上" = 落在**零支**上）。
        //   不许静默成功：从前那条兜底会**合成**一支老家形状的住户，于是这一笔会去写
        //   `<家>/env/model_binding.json` —— 一份**不属于任何人**的绑定，而快照还会报"已指定"。
        //   ⇒ 明确拒绝（稳定码 `no_resident`），页面那一侧本来也不摆这颗按钮。
        //   ⚠ **2026-10-14**：这一条只对"**没点名**"的那一笔成立 —— 点了名的（哪怕这台机器上
        //   一支住户都还没有）有目标：写在她将来会住的那个环境根上（"选择先于接上"）。
        if (wanted === null && envDirsOf().length === 0) {
          respond(res, 409, { error: ERRORS.NO_RESIDENT })
          return
        }
        const targets = writeTargetsOf(wanted)
        const source = typeof parsedBody.source === 'string' ? parsedBody.source.trim() : ''
        // ★ **支级覆盖那两格**（2026-10-14）："用哪个型号 / 什么思考档"**归这一支**（运行时一直
        //   认它们：`agent/src/model_source.rs` 的 `apply_binding_overrides`），而在此之前
        //   **没有任何写者**能写它们。三态：**不给** = 不动（重新绑定时沿用旧值）；`null` = 清掉；
        //   字符串 = 写成它。形状在这一格先判（fail-closed，不许写一份宿主会拒绝启动的文件）。
        //
        //   ⚠ **只在点名 `agentId` 时接受**：那两格是**支级事实**，而"没点名 = 落在每一支上"
        //   的家级默认把它们写成"每一支都用同一个型号"，那不是任何人要的（不猜 ⇒ 明确拒）。
        const hasOverrides = parsedBody.model !== undefined || parsedBody.reasoningEffort !== undefined
        if (hasOverrides && wanted === null) {
          respond(res, 400, { error: ERRORS.BAD_REQUEST })
          return
        }
        if (hasOverrides && !source) {
          // "解除绑定" + 覆盖两格 = 自相矛盾（文件要删了，覆盖没有落点）⇒ 明确拒，不静默吞。
          respond(res, 400, { error: ERRORS.BAD_REQUEST })
          return
        }
        let overrides = {}
        if (hasOverrides) {
          if (parsedBody.model !== undefined && parsedBody.model !== null) {
            if (!modelSources.modelShapeOk(parsedBody.model)) {
              respond(res, 400, { error: ERRORS.INVALID_MODEL })
              return
            }
          }
          if (
            parsedBody.reasoningEffort !== undefined &&
            parsedBody.reasoningEffort !== null &&
            !modelSources.REASONING_EFFORTS.includes(parsedBody.reasoningEffort)
          ) {
            respond(res, 400, { error: ERRORS.INVALID_EFFORT })
            return
          }
          overrides = {
            model: parsedBody.model === undefined ? undefined : parsedBody.model,
            reasoningEffort:
              parsedBody.reasoningEffort === undefined ? undefined : parsedBody.reasoningEffort,
          }
        }
        if (!source) {
          try {
            clearBindingEverywhere(targets)
          } catch {
            respond(res, 500, { error: ERRORS.SAVE_FAILED })
            return
          }
          respond(res, 200, { ...snapshot().payload })
          return
        }
        const list = modelSources.readList(homeDir)
        if (list.state === 'unreadable') {
          respond(res, 500, { error: ERRORS.READ_FAILED })
          return
        }
        if (!list.entries.some((entry) => entry.id === source)) {
          respond(res, 404, { error: ERRORS.ENTRY_NOT_FOUND })
          return
        }
        try {
          writeBindingEverywhere(source, targets, overrides)
        } catch {
          respond(res, 500, { error: ERRORS.SAVE_FAILED })
          return
        }
        respond(res, 200, { ...snapshot().payload })
        return
      }

      // ---- `/home-config` 的写：POST 新建一条 / PUT 更新一条（`bind` 决定要不要绑它）----
      const before = snapshot()
      if (before.list.state === 'unreadable') {
        respond(res, 500, { error: ERRORS.READ_FAILED })
        return
      }
      const picked = pickEntryId(parsedBody, {
        list: before.list,
        bound: before.payload.bound,
        create: creating,
      })
      if (!picked.ok) {
        respond(res, picked.error === ERRORS.ENTRY_NOT_FOUND ? 404 : 400, { error: picked.error })
        return
      }
      // "留空 key 沿用"只对**更新**成立（改哪一条就沿用哪一条的钥匙，不用重贴）；
      // 新建没有可沿用的东西 ⇒ 必须给 key，否则只会写出一条用不了的条目。
      const checked = validateDeclaration(parsedBody, picked.previous?.api_key ?? null)
      if (!checked.ok) {
        respond(res, 400, { error: checked.error })
        return
      }
      const {
        endpoint,
        credential,
        model,
        contextWindow: givenWindow,
        protocol,
        name: givenName,
        bind,
        provider,
        vision,
      } = checked.declaration
      // ---- 窗口（TASK_77 回炉③）：不是配置，是体检 ----
      //
      // 留空 ⇒ **先试连再落盘**（窗口要进文件，所以不能等写完了才问）；问到就落盘、问不到才回头
      // 要人填。给了窗口 ⇒ 顺序与从前**一字未改**：先落盘、再试连读数（"试连失败不拦保存"
      // 那条在给了窗口这条路上照旧成立）。
      let contextWindow = givenWindow
      let windowSource = givenWindow === null ? null : 'given'
      let result = null
      if (contextWindow === null) {
        result = await probeFull(endpoint, credential, model, provider)
        if (result.contextWindow !== null) {
          contextWindow = result.contextWindow
          windowSource = 'probed'
        } else {
          // ★ 探测没读到 ⇒ **先查兜底表**（限家 + 精确命中），再谈"要不要人填"。
          // 表里那个值来自社区维护的公开表 ⇒ `windowSource` 如实标 `table`，
          // **不冒充"对方给的"**（与「预设 ≠ 已配置」同一条纪律：帮填 ≠ 权威值）。
          const fromTable = windowTable.windowFor(provider, model)
          if (fromTable !== null) {
            contextWindow = fromTable
            windowSource = 'table'
          }
        }
        if (contextWindow === null) {
          // 两条路都空 ⇒ 这时才报错。★ **病因分开**：钥匙被对方拒（401/403）与该接口不给窗口
          // 是两件事，用户该做的**相反** —— 前者去换钥匙（手填窗口照样连不上），后者才该手填窗口。
          // 试连读数一并交回去：人要看的正是"为什么没问到"。
          const rejected = result.status === 401 || result.status === 403
          respond(res, 400, {
            error: rejected ? ERRORS.CREDENTIAL_REJECTED : ERRORS.WINDOW_UNAVAILABLE,
            probe: result,
          })
          return
        }
      }
      const name =
        typeof givenName === 'string' && givenName.trim()
          ? givenName.trim()
          : (picked.previous?.name ?? model)
      const upserted = modelSources.upsertEntry(homeDir, {
        id: picked.id,
        name,
        endpoint,
        api_key: credential,
        model,
        context_window: contextWindow,
        protocol,
        // **只在这三态里的"说了"那一态才带上它**（2026-10-17）：来件**没说**时这一格
        // **整个不出现** ⇒ `upsertEntry` 从旧条目原样带过来（老页面保存不许删掉它）。
        // ⚠ 写成 `vision: vision ?? false` 就毁了这条——那会把"没说"压成"说了不支持"。
        ...(vision === undefined ? {} : { vision }),
      })
      if (!upserted.ok) {
        respond(res, 500, { error: ERRORS.SAVE_FAILED })
        return
      }
      // 清单被这道门改过了 ⇒ 记一条（这一刻它的字节就是刚写下去的那一份）。
      witnessWrite(modelSources.listPath(homeDir))
      // **绑不绑由 `bind` 说**：缺省 true = 老页面那句"保存 = 用它"；清单页发 false
      // （它只管清单——"用哪一条"是模型房间的事）。
      if (bind) {
        try {
          writeBindingEverywhere(picked.id)
        } catch {
          respond(res, 500, { error: ERRORS.SAVE_FAILED })
          return
        }
      }
      // 试连：**保存已经落定了**，这一步只出读数（失败不拦保存，也不回滚）。
      // （留空那条路上面已经探过一次，不重复打第二枪。）
      if (result === null) result = await probeFull(endpoint, credential, model, provider)
      respond(res, 200, {
        ...snapshot().payload,
        entryId: picked.id,
        creating: picked.creating,
        windowSource,
        probe: result,
      })
    })
  }
  // 传输层按准入走：模式二给了 `tls` ⇒ HTTPS（裁决：非 TLS 不开远程）；模式一照旧 HTTP。
  const server = tls ? https.createServer(tls, handler) : http.createServer(handler)
  return server
}

module.exports = {
  createConfigServer,
  probeNative,
  NATIVE_PROBES,
  NATIVE_LIST_LIMIT,
  originOf,
  kindsFromDashscope,
  probeModels,
  modelsUrl,
  windowFromModels,
  modelsFromBody,
  catalogEntries,
  WINDOW_FIELDS,
  MODEL_LIST_LIMIT,
  validateDeclaration,
  validateProbe,
  pickEntryId,
  agentIdShapeOk,
  originAllowed,
  DEFAULT_CONFIG_PORT,
  PROTOCOLS,
  PROBE_PATH,
  CATALOG_PATH,
  CONTROL_PATH,
  ERRORS,
  BODY_LIMIT_BYTES,
  MODEL_BODY_LIMIT_BYTES,
  PROBE_TIMEOUT_MS,
  // 清单/绑定的形状与迁移那份权威（刀2b）：门面转发出去，`up.js` 与用例都用它。
  modelSources,
  // 白名单 env 复用状态口那一份：那是"操作系统必需品"这一类事实的唯一权威（同一纪律）。
  serveEnv: statusServer.serveEnv,
}
