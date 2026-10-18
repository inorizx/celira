// 中性稳定错误文案测试（node --test + 类型剥离）。
//
// 被测对象是 `src/api/contractErrors.ts`：吃的是 **Core 稳定错误码**
// （`ApiError.code`），Host 线只是同一个字段的另一个来源。这些映射**与宿主
// 无关**，调用点也不在 host 分支里（撤回 / 发送失败 / 上传失败，core 也走）。
//
// TASK_64 删除刀 D 的覆盖保护：下列用例原先散在 host-*.test.ts（整份在删除
// 名单上）里，**逐字搬出**，以免中性模块的覆盖随 Host 线一起清零。用例文本、
// 断言、注释一字未改。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  agentCreateErrorText,
  agentRevokeErrorText,
  apiErrorText,
  errorText,
  roomActionErrorText,
  SESSION_EXPIRED_TEXT,
} from '../src/api/contractErrors.ts'

/** 源级判据要读源文件（本仓 client 测试没有 jsdom，见 `no-silent-red-path.test.ts` 头注）。 */
const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

/** 被测表本身（源级判据用它取 `case` 标签，别手抄清单）。 */
const errorSource = read('../src/api/contractErrors.ts')

/** 表里全部 `case` 标签（`attachment_download` 那是前缀分支，不是 case）。 */
const mappedCases = [...errorSource.matchAll(/case '([^']+)'/g)].map((m) => m[1])

/**
 * im 的错误词表（`protocol/error.rs` 的 `pub mod codes`）；**判据自检**：读不到就抛。
 *
 * 为什么读源码而不是抄一份：抄一份的下场就是本刀修掉的那个 bug（client 自己编了
 * 4 个码，抄的那份清单跟着一起错，谁也发现不了）。
 */
function imErrorCodes(): Set<string> {
  const source = read('../../im/server/src/protocol/error.rs')
  const codes = new Set(
    [...source.matchAll(/pub const [A-Z_]+: &str = "([a-z_]+)"/g)].map((m) => m[1]),
  )
  // 下限是**判据自己的守门员**：路径/格式一变，这里会红，而不是让下面几条静默空跑。
  // ⚠ 2026-09-28：70 → 65。真实读数从 73 掉到 **67** —— 少的 6 个是随
  // **Agent 服务层退役**删除的服务错误码（`publication_invalid` / `publication_stale` /
  // `access_conflict` / `access_not_found` / `service_access_required` /
  // `service_not_accepting`）。**这是减法，不是覆盖缺口**：码没了，判据的下限跟着落。
  // ⚠ 2026-09-28 第二刀：65 → 60。真实读数 67 → **62** —— 少的 5 个是随
  // **Completion/ACK 整体退役**删除的投递错误码（`completion_conflict` /
  // `completion_source_inactive` / `completion_source_recalled` /
  // `ack_without_completion` / `ack_out_of_order`）。同样是减法。
  // ⚠ 2026-09-28 第三刀：60 → 55。真实读数 62 → **56** —— 少的 6 个是随
  // **Live State 按主体形状重做**删除的执行面码（`event_not_found` 只服务
  // 旧的 activity 写入口；`activity_conflict` / `activity_stale` /
  // `activity_stream_busy` / `activity_body_not_allowed` /
  // `live_state_limit_exceeded`）。判据下限跟着落，不是覆盖缺口。
  assert.ok(codes.size >= 55, `im 词表读到的码数（实测 ${codes.size}）—— 读不到就是判据坏了`)
  assert.ok(codes.has('invalid_session') && codes.has('authentication_required'))
  return codes
}

test('agentCreateErrorText is stable and never leaks secrets', () => {
  const err = (code: string) => ({ code, message: 'sk-agent-secret', request_id: 'sk-rid' })
  const text = agentCreateErrorText(err('invalid_request'))
  assert.equal(text, '名字不符合规则（最长 80 个字符，不能为空），请修改后重试。')
  assert.ok(!text.includes('sk-'), 'never leaks server body')
  assert.equal(
    agentCreateErrorText(err('idempotency_conflict')),
    '已有一个相同的创建请求正在处理，请刷新后重试。',
  )
  const unknownText = agentCreateErrorText(err('internal_secret'))
  assert.equal(unknownText, '创建失败，请稍后重试。')
  assert.ok(!unknownText.includes('sk-'))
})

test('agentRevokeErrorText 稳定且不泄露正文（agent_not_found 有专名文案）', () => {
  const err = (code: string) => ({ code, message: 'sk-revoke-secret', request_id: 'sk-rid' })
  const notFound = agentRevokeErrorText(err('agent_not_found'))
  assert.equal(notFound, '没有找到这个 Agent，可能已被撤销。')
  assert.ok(!notFound.includes('sk-'), 'never leaks server body')
  assert.equal(agentRevokeErrorText(err('storage_unavailable')), '暂时无法写入，请稍后重试。')
  assert.equal(agentRevokeErrorText(err('agent_not_active')), '这个 Agent 已失效，当前不能撤销。')
  assert.equal(agentRevokeErrorText(err('invalid_session')), `${SESSION_EXPIRED_TEXT}。`)
  const unknown = agentRevokeErrorText(err('internal_secret'))
  assert.equal(unknown, '撤销失败，请检查连接后重试。')
  assert.ok(!unknown.includes('sk-'))
  // 非对象错误（网络层抛字符串）也不炸、不泄露。
  assert.equal(agentRevokeErrorText('boom'), '撤销失败，请检查连接后重试。')
})

test('Host room error text is stable, single-opinion and never leaks secrets', () => {
  const t = (code: string) => ({ code })
  assert.equal(roomActionErrorText(t('room_not_found')), '房间不存在或无权访问')
  assert.equal(roomActionErrorText(t('member_not_found')), '成员不存在或无权操作')
  assert.equal(roomActionErrorText(t('owner_not_removable')), '群主不能被移除')
  assert.equal(roomActionErrorText(t('owner_transfer_required')), '转让群主后才能退出')
  assert.equal(roomActionErrorText(t('invalid_transfer_target')), '只能转让给另一位成员')
  assert.equal(roomActionErrorText(t('idempotency_conflict')), '重复请求冲突，请重试')
  // 未知分类一律通用文案；绝不回显 code/status/服务端正文。
  // ⚠ 通用文案**不许点名动作** —— 撤回 / 邀请 / 建绑定 / 加联系人都会落到这里，
  // 点名"发送"就是说反了（它曾经是「发送失败，请重试」，已改成中性）。
  const unknown = roomActionErrorText(new Error('raw server body: sk-secret'))
  assert.equal(unknown, '操作失败，请重试')
  assert.ok(!unknown.includes('sk-secret'))
  // 稳定文本中绝不出现幂等 key / 路径 / 会话秘密。
  const all = [roomActionErrorText(t('room_not_found')), roomActionErrorText(t('owner_transfer_required'))].join('')
  assert.ok(!all.includes('owner-transfer'))
  assert.ok(!all.includes('invite-key'))
})

test('apiErrorText：**每个分类**（从源码取，不手抄清单）都给稳定人话且不泄露', () => {
  // ⚠ 这一条原先手抄了一份清单 —— 4 条合成死码就是这样在里面躺了很久（没有任何
  // 生产者，谁也没发现它们匹配不上）。改成**从源文件取 `case` 标签**：新增映射
  // 自动进判据，删掉的也不会留在清单里假装被测。
  assert.ok(mappedCases.length >= 25, `从源码取到的分类数（实测 ${mappedCases.length}）`)
  for (const code of mappedCases) {
    const text = apiErrorText(code)
    assert.ok(text.length > 2, `stable text for ${code}`)
    assert.ok(!text.includes('undefined'), `no raw undefined: ${code}`)
    assert.ok(!/[\da-f]{8}-[\da-f]{4}/.test(text), `no uuid-like token in text for ${code}`)
    assert.ok(!text.includes('sk-'), `no secret-looking text for ${code}`)
    assert.ok(!text.includes('/v1/'), `no path in text for ${code}`)
  }
  assert.equal(apiErrorText('attachment_download_failed'), '附件下载失败，请重试')
  assert.match(apiErrorText('attachment_download_failed'), /重试/)
  const fallback = apiErrorText(undefined)
  assert.ok(fallback.length > 2, 'default fallback text')
  assert.ok(!fallback.includes('private-message'))
  assert.ok(!fallback.includes('/srv/celira/'))
  assert.ok(!fallback.includes('token='))
})

// ── 词汇对账（TASK_78 刀 1）：**本表的每个分类都必须是 im 的真词** ─────────
//
// 这是这一刀要防的那类 bug 的机械判据：client 自己编一个码（`core_session_invalid`
// 那族的形状），它就永远匹配不上、把该说的话吞掉，而**没有任何判据会响**。
// 词表的权威在 im（`protocol/error.rs` 的 `pub mod codes`，注释明写"新增错误码必须
// 在此登记"），所以这里直接把那份词读进来对表。

test('本表的每个分类都是 im 的真词（唯一白名单见下）', () => {
  const imCodes = imErrorCodes()

  // **白名单只有两条**，每条都点名它**唯一的生产者**（否则它就是一个幽灵码）：
  // 1. `attachment_download_failed` 不是 im 码，而是 `lib/attachmentDownload.ts` 在
  //    "没有任何可用下载路径"时自己用来取文案的分类（唯一生产者就是那个调用点，
  //    `no-silent-red-path.test.ts` 钉着它）。
  // 2. `transport_unreachable`（2026-10-08）不是 im 码，而是**出网那一层**自己的分类：
  //    `api/transport.ts` 的 `sendRequest` 把"发不出去"折成 `TransportFailure`
  //    （唯一生产者就是那一处；`tests/transport.test.ts` 用**行为**钉着它 —— 拿一份
  //    会抛错的实现喂进去，断言折出来的分类就是这个码）。
  //    ⚠ 它必须存在：在这一刀之前，"发不出去"**根本没有名字**，于是壳的 Rust 侧抛的
  //    reqwest 原文（`error sending request for url (…)`）一路原样渲染到了登录页上。
  const CLIENT_OWN = new Set(['attachment_download_failed', 'transport_unreachable'])

  const offenders = mappedCases.filter((code) => !imCodes.has(code) && !CLIENT_OWN.has(code))
  assert.deepEqual(
    offenders,
    [],
    `这些分类 im 根本不发（会永远落到通用文案）：${offenders.join(', ')} —— ` +
      '要么删，要么先去 im 立词（本仓 client 不许自己编码）',
  )
})

test('另一张文案表（`presentation.ts`）同样不许自己编码', () => {
  // 口径与上一条一致，只是换了文件：族二族三是**同一个 bug 的另一半**（族二原先有
  // 3 条 im 不发的码，其中 `access_not_active` / `service_direct_room_conflict`
  // 连近似真词都没有）。
  //
  // ⚠ 2026-09-28：原为**两张**表（`service.ts` / `presentation.ts`）——`service.ts`
  // 随 Agent 服务层退役删除，这里只剩 presentation 一张。
  const imCodes = imErrorCodes()
  for (const file of ['../src/api/presentation.ts']) {
    const cases = [...read(file).matchAll(/case '([^']+)'/g)].map((m) => m[1])
    assert.ok(cases.length >= 5, `${file} 取到的分类数（实测 ${cases.length}）`)
    const offenders = cases.filter((code) => !imCodes.has(code))
    assert.deepEqual(offenders, [], `${file} 里 im 不发的分类：${offenders.join(', ')}`)
  }
})

test('★ 出网发不出去 ⇒ 固定人话（**库的原文一个字都不许上屏**）', () => {
  // 现场（`app/README.md` §5 第 15 行）：登录页上曾经显示的就是 reqwest 的原文
  // `error sending request for url (http://127.0.0.1:8180/v1/me)` —— 那是库的话，不是用户的话。
  const text = apiErrorText('transport_unreachable')
  assert.equal(text, '连不上这个世界 —— 地址或端口不对，或者那个服务没在跑')
  assert.equal(errorText({ code: 'transport_unreachable' }), text, '经通用入口也要走到同一条')
  // 它要说的是**下一步**（地址 / 端口 / 服务在不在跑），不是"重试"—— 地址错了重试一万次也一样。
  assert.match(text, /地址|端口/)
  assert.ok(!/重试/.test(text), '地址不对时"重试"是空话')
  // ★ 原文与地址都不许从这一格漏出去：一个裸 `Error`（没有稳定分类）只能落中性兜底。
  const leaked = errorText(new Error('error sending request for url (http://127.0.0.1:8180/v1/me)'))
  assert.ok(!leaked.includes('error sending request'), `库的原文不许上屏：${leaked}`)
  assert.ok(!leaked.includes('8180'), `地址也不许从这一格漏出去：${leaked}`)
})

test('会话失效（401）**三张表**都给"重新登录"的话，且共用同一句', () => {
  // im 的 401 真词（`protocol/error.rs:20/22`）—— 这条是验收要求的"行为用例"。
  for (const code of ['authentication_required', 'invalid_session']) {
    assert.equal(apiErrorText(code), SESSION_EXPIRED_TEXT, code)
    assert.equal(roomActionErrorText({ code }), SESSION_EXPIRED_TEXT, code)
    assert.equal(errorText({ code }), SESSION_EXPIRED_TEXT, code)
    assert.equal(agentCreateErrorText({ code }), `${SESSION_EXPIRED_TEXT}。`, code)
    assert.equal(agentRevokeErrorText({ code }), `${SESSION_EXPIRED_TEXT}。`, code)
    // 说的是"重新进入"，不是"重试"—— 会话过期时重试同一份凭据不会有别的结果。
    assert.ok(!SESSION_EXPIRED_TEXT.includes('重试'), '会话过期不该说成"重试"')
    assert.match(SESSION_EXPIRED_TEXT, /重新/)
  }
})

// ── 审计 A3：那 7 处原先把 `code (HTTP status)` 直接打给用户 ──
//
// 现场：`MyAgentsPage`（建绑定/签发凭据/撤绑定/撤凭据）、`NewContactPage`（加联系人/建 Agent）、
// `ContactDetailCard`（发起直聊）原先各自 `e.message`，而 `ApiError.message` 就是
// `` `${code} (HTTP ${status})` `` ⇒ 用户看到的是 `backend_binding_exists (HTTP 409)`。
// 原注释自认"有意"，理由是它**绝不回显服务端正文/key/路径** —— 那个属性是对的，
// 但代价不该是机器话（本表"只吃稳定分类码"这条判据已经保证了同一件事）。
test('审计 A3：绑定/凭据与联系人那 7 个码都有对口人话（原先一条映射都没有）', () => {
  const expected: ReadonlyArray<readonly [string, string]> = [
    ['backend_binding_exists', '这个 Agent 已经有一个后端绑定了'],
    ['backend_binding_not_found', '后端绑定不存在或已被撤销'],
    ['backend_credential_not_found', '凭证不存在或已被撤销'],
    ['credential_state_conflict', '凭证状态已变化，请刷新后重试'],
    ['human_not_found', '找不到这位用户'],
    ['contact_not_found', '联系人不存在'],
    ['contact_not_accepted', '对方还没有接受你的联系人请求'],
  ]
  for (const [code, text] of expected) {
    assert.equal(apiErrorText(code), text, code)
    assert.equal(errorText({ code }), text, `${code} 也要能经 errorText 走到同一条`)
    // 判据：文案**不许是机器话**（分类码的下划线形态 / HTTP 状态码），
    // 也不许落进通用兜底那句 —— 它说的是"发送失败"，对这几个动作是说反了。
    assert.ok(!/_/.test(text) && !/HTTP\s*\d/.test(text), `不许是机器话：${text}`)
    assert.notEqual(text, '操作失败，请重试', `${code} 落进了通用兜底 ⇒ 说明它没被补进表里`)
  }
})
