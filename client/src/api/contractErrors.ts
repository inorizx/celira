// 稳定错误分类 → 人类可读文案（**与宿主无关**；TASK_57 从 `hostUi.ts` 搬出来）。
//
// 为什么搬：这些函数吃的是 **Core 稳定错误码**（`ApiError.code`），Host 线只是同一个
// 字段的另一个来源。留在 `hostUi.ts` 里会让人以为它们属于 Host 模式 ——
// 而它们的调用点根本不在 host 分支里（撤回 / 发送失败 / 上传失败，core 也走）。
//
// 纪律（原样保留，一字未动）：
// - 只接受稳定白名单分类并输出固定文案；**绝不携带 key / 服务端正文 / 路径**；
// - 认不出的分类一律落到中性通用文案，绝不回显服务端内容；
// - 以**鸭子类型**提取稳定分类（不引入宿主传输层依赖，可在 `node --test` 直测）。

/**
 * **会话失效的唯一一句人话**（HTTP 401）。
 *
 * im 在 401 上发的是 `authentication_required` / `invalid_session`
 * （`im/server/src/protocol/error.rs:20/22`）——会话过期时用户要做的是**重新进入**，
 * 不是"重试"。两张文案族（本文件、`presentation.ts`）共用这一句，
 * 免得同一个事实有两句人话。
 * （2026-09-28：原第三张 `service.ts` 已随服务层退役删除。）
 *
 * ⚠ 这里原先映射 `core_session_invalid`：一个 **im 从不发出、client 也没有生产者**的码
 * ⇒ 401 一路落到通用文案，把"该重新登录"说成了别的事。已剔。
 */
export const SESSION_EXPIRED_TEXT = '登录已失效，请重新进入'

/**
 * 稳定错误分类 → 人类可读稳定文本（绝不携带 key/原文/路径）。
 *
 * 每条 `case` 注明 im 出处（`im/server/src/protocol/error.rs:<行>`）：**词表的权威
 * 在 im**，本表只是那份词的中文人话。
 *
 * ⚠ 本表**不含** client 自己编的合成码（曾是 `network_failure` / `core_unavailable` /
 * `core_session_invalid` / `core_write_failed`）：实测它们**没有任何生产者** ——
 * `fetchJson` 只把 im 的 `error.code` 原样抛出，断网时 `fetch` 抛的是 `TypeError`。
 * 匹配不上的分支等于没写（还会让人以为"网络失败有专门文案"），已删。
 *
 * ✅ **2026-10-08 补：那个生产者来了**（当年那句"要恢复'网络层的人话'，得先有真正的生产者，
 * 那是另一刀的事"—— 这一刀就是它）。出网那一层（`transport.ts` 的 `sendRequest`）
 * 现在把"发不出去"折成 `TransportFailure`，带一个稳定分类 `transport_unreachable`；
 * 于是下面那条 `case` **有真东西喂它**了。**只补这一条**：其余四个旧合成码仍然没有生产者，
 * 不许顺手复活（那会重新变成"写了却恒不命中"的装饰）。
 */
export function apiErrorText(code: string | undefined): string {
  // **带后缀的家族先折到稳定分类上**（TASK_72 追补）：`fetchBlob` 抛的是
  // `attachment_download_<status>`（`client.ts` 用 `codePrefix` 拼状态码）。不折的话
  // 它会落进 `default` 说成「**发送**失败」—— 把"下载失败"说成"发送失败"是**说反了**。
  //
  // ⚠ **只有下载这一族如此**：`uploadMultipart` **没有** `codePrefix` 参数，它抛的是
  // im 信封里的真码 ⇒ 曾在这里的 `attachment_upload` 前缀与 `attachment_upload_failed`
  // 都是**没有生产者的幽灵**，已删（上传失败的文案由 im 真码经本表给出）。
  if (typeof code === 'string') {
    if (code.startsWith('attachment_download')) return '附件下载失败，请重试'
  }
  switch (code) {
    // ── 会话 / 基础设施 ──
    case 'authentication_required': // im :20（401）
    case 'invalid_session': // im :22（401）
      return SESSION_EXPIRED_TEXT
    // ⚠ **client 合成的码，但它有真正的生产者**（2026-10-08，见文件头那段）：出网那一层
    // （`transport.ts` 的 `sendRequest`）把"发不出去"折成 `TransportFailure`。
    // 它说的是"**够不着那个服务**"（不是"它说不行"）—— 所以要给的是**下一步**：
    // 地址/端口对不对、那个服务在不在跑。原文（reqwest 的 `error sending request for url (…)`
    // 或浏览器的 `Failed to fetch`）**一个字都不许上屏**，它进控制台。
    case 'transport_unreachable':
      return '连不上这个世界 —— 地址或端口不对，或者那个服务没在跑'
    case 'storage_unavailable': // im :18（503）
      return '服务端数据暂时不可用，请稍后重试'
    case 'internal_error': // im :16（500）
      return '服务端内部错误，请稍后重试'

    // ── 消息 ──
    case 'invalid_request': // im :24
      return '请求不合法，请检查消息正文或附件'
    case 'invalid_message_body': // im :44
      return '消息内容不合法（不能为空、过长或含不支持的字符）'
    case 'invalid_mention': // im :58
      return '提及的对象不可用，请重新选择要 @ 的对象'
    case 'invalid_reply_target': // im :88
      return '引用目标不可用，无法回复'
    case 'message_not_found': // im :91
      return '消息不存在或无权撤回'
    case 'message_recall_window_expired': // im :94
      return '已超过可撤回时间（发送后 2 分钟内）'
    case 'payload_too_large': // im :34（请求体超限；附件超限也走它）
      return '内容过长，超过了服务端上限'

    // ── 幂等 ──
    case 'idempotency_conflict': // im :32
      return '重复请求冲突，请重试'
    case 'invalid_idempotency_key': // im :28
      return '请求重复标识不合法，请重试'
    case 'idempotency_key_required': // im :26
      return '请求缺少重复标识，请重试'

    // ── 房间 / 成员 ──
    case 'room_not_found': // im :36
      return '房间不存在或无权访问'
    case 'member_not_found': // im :75
      return '成员不存在或无权操作'
    case 'owner_not_removable': // im :77
      return '群主不能被移除'
    case 'owner_transfer_required': // im :79
      return '转让群主后才能退出'
    case 'invalid_transfer_target': // im :81
      return '只能转让给另一位成员'

    // ── 联系人 ──
    case 'human_direct_room_exists': // im :72
      return '与这位联系人的私聊已经存在'
    case 'human_not_found': // im :61（仍只用于 Human-only 路径）
      return '找不到这位用户'
    case 'participant_not_found': // im :66（批次三第二刀：关系面目标可以是任意主体）
      return '找不到这位用户'
    case 'contact_not_found': // im :68
      return '联系人不存在'
    case 'contact_not_accepted': // im :70
      return '对方还没有接受你的联系人请求'

    // ── Agent ──
    case 'agent_not_active': // im :42
      return '这个 Agent 已失效，当前不能操作'

    // ── Agent 的后端绑定与凭证 ──
    // 这四个码原先**没有任何映射** ⇒ 「我的 Agent」页那 4 个动作失败时，界面直接把
    // `code (HTTP status)` 这种机器话打给用户（审计 A3）。补在这里之后，那 4 处改成
    // 走 `errorText(e)` 就能得到对口的人话，**且依然不回显服务端正文 / key / 路径** ——
    // 本表只吃稳定分类码，这正是它与裸 `e.message` 的关键差别（原注释担心的那件事
    // 在这里仍然成立，只是不必再拿机器话当代价）。
    case 'backend_binding_exists': // im :52
      return '这个 Agent 已经有一个后端绑定了'
    case 'backend_binding_not_found': // im :48
      return '后端绑定不存在或已被撤销'
    case 'backend_credential_not_found': // im :50
      return '凭证不存在或已被撤销'
    case 'credential_state_conflict': // im :54
      return '凭证状态已变化，请刷新后重试'

    // ── 附件 ──
    case 'attachment_not_found': // im :102
      return '附件不存在或无权访问'
    case 'invalid_attachment': // im :99
      return '附件不可用，请重新选择后重试'
    case 'invalid_attachment_metadata': // im :96
      return '附件信息异常，请重新上传'
    // ⚠ 这一条**不是** im 码：`lib/attachmentDownload.ts` 在"没有任何可用下载路径"时
    // **自己**用它来取这句文案（唯一生产者就是那个调用点，有守卫钉着）。
    case 'attachment_download_failed':
      return '附件下载失败，请重试'

    // ── 通话（`CORE_V1_CALL.md` §7 的稳定码；2026-10-09 接线时补）──
    // 这五个码原先在本表**一条映射都没有** ⇒ 界面上会直接显示 `code (HTTP status)`
    // 那种机器话（`ApiError.message` 的形状）。逐条给的是**用户能怎么办**，
    // 不是"哪一层拒了"：通话这一面绝大多数失败都是"对方那边的事"，说清这一点比说码有用。
    case 'call_party_busy': // im（409）：任一方已在另一通 active Call 里
      return '对方正在另一通电话里，稍后再试'
    case 'call_not_found': // im（404）：未知 / 非本 Call / 非参与者（**不泄露存在性**）
      return '这通电话已经结束或不存在了'
    case 'call_state_conflict': // im（409）：当前状态不允许该动作（含 ended 后再操作、信令方向错）
      return '这通电话的状态已经变了，请刷新后重试'
    case 'call_payload_invalid': // im（400）：SDP/ICE 越界、禁 NUL、形状错
      return '通话协商数据不合法，请重新发起'
    case 'call_state_limit_exceeded': // im（503）：进程内有界资源上限
      return '这台机器上的通话暂时排满了，稍后再试'

    // ⚠ 兜底文案**不许点名某个动作**。它服务的是"这个码本表认不出"这一情形，而调用它的
    // 动作可能是撤回、邀请、建绑定、加联系人…… 原先写「发送失败，请重试」是**说反了**
    // （刀 1 报裁第①条；审计 A3 之后触发面还变大了 —— 那 7 处新调用点都会落到这里）。
    // 改成中性文案，与 `agentCreateErrorText` / `agentRevokeErrorText` 两族的兜底同一口径
    // （「创建失败…」/「撤销失败…」各自点名自己的域 —— 那是**说对了**；只有通用族没有域可点）。
    default:
      return '操作失败，请重试'
  }
}

/** 从任意错误对象里鸭子类型取稳定分类（不依赖任何传输层运行时）。 */
function codeOf(error: unknown): string {
  return error && typeof error === 'object' && 'code' in error
    ? String((error as { code?: unknown }).code ?? '')
    : ''
}

/**
 * **任意错误对象 → 人话**（通用入口；`apiErrorText` 是单一事实源）。
 *
 * 与 `roomActionErrorText` 是同一件事，只是名字不绑"房间动作"—— 附件下载这类
 * **不是房间动作**的路也要说人话（TASK_72 追补"红路径不许静默"），共用这一条路径。
 * 认不出的错误对象一律落通用文案，绝不回显服务端正文。
 */
export function errorText(error: unknown): string {
  return apiErrorText(codeOf(error) || undefined)
}

/**
 * Room Management（P2）写路径稳定错误文案：建群/邀请/移除/转让/退群。
 * 只透传 Core 稳定分类（`apiErrorText` 单一事实源）；绝不携带 key/原文/路径。
 */
export function roomActionErrorText(error: unknown): string {
  return errorText(error)
}

/**
 * Agent Create（P3）写路径稳定错误文案；绝不携带 key/原文/路径/Core body。
 * invalid_request 覆盖非法名/超长/缺 key 等全部名称与请求规则失败（名称
 * 规则在 Core 唯一裁决，本层不泄露具体原因）。未知分类一律通用文案。
 */
export function agentCreateErrorText(error: unknown): string {
  switch (codeOf(error)) {
    case 'authentication_required': // im :20（401）
    case 'invalid_session': // im :22（401）
      // ⚠ 这一族以句号收尾（`apiErrorText` 那族不收）⇒ 共用常量时补一个句号。
      return `${SESSION_EXPIRED_TEXT}。`
    case 'invalid_request': // im :24
      return '名字不符合规则（最长 80 个字符，不能为空），请修改后重试。'
    case 'idempotency_conflict': // im :32
      return '已有一个相同的创建请求正在处理，请刷新后重试。'
    case 'storage_unavailable': // im :18（503）
    case 'internal_error': // im :16（500）
      return '暂时无法写入，请稍后重试。'
    default:
      return '创建失败，请稍后重试。'
  }
}

/**
 * Agent 撤销（core 直连 `DELETE /v1/agents/{id}`）的稳定文案。
 *
 * **只有这一份映射**：直连 `ApiError.code` 就是分类字段，所以不按模式分叉文案。
 * `agent_not_found` 是「没找到」而不是「撤销失败」
 * ——unknown / 已撤销 / 非本人在 im 服务端 早已统一为同一分类（存在性不被额外
 * 泄露）；认不出的分类一律落到中性文案，绝不回显服务端正文。
 */
export function agentRevokeErrorText(error: unknown): string {
  switch (codeOf(error)) {
    case 'authentication_required': // im :20（401）
    case 'invalid_session': // im :22（401）
      return `${SESSION_EXPIRED_TEXT}。`
    case 'agent_not_found': // im :30
      return '没有找到这个 Agent，可能已被撤销。'
    case 'agent_not_active': // im :42
      return '这个 Agent 已失效，当前不能撤销。'
    case 'storage_unavailable': // im :18（503）
    case 'internal_error': // im :16（500）
      return '暂时无法写入，请稍后重试。'
    default:
      return '撤销失败，请检查连接后重试。'
  }
}
