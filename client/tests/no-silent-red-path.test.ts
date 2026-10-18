// **红路径不许静默**（TASK_72 追补）——一条纪律、两件落地：撤回失败 / 附件下载失败。
//
// 这两件的形状**一模一样**，所以判据也放在同一个文件里：
//   · 句子里早就有"稳定分类 → 人话"的映射（`api/contractErrors.ts`），
//   · 但那条路上**没有消费者** —— `catch {}` / `void downloadAttachment(...)` 把失败吞了，
//   · 于是"点了没反应"，而映射函数成了**死映射**。
//
// 落地的形状也一致：**失败时把已有那句人话显示出来**（`role="alert"`），不制造假成功。
//
// 判据三类，缺一类都不算修好：
//  ① 人话**逐字**对得上（行为判据，直接调映射函数）；
//  ② 那条路**真的把返回值用上了**（撤回：catch 里调映射；下载：消费返回值）；
//  ③ 显示出来了（`{… && (…)}` + `role="alert"`）。
//
// ⚠ ②③ 是**源级守卫**：这个仓库的 client 测试是 `node --test` + TS 类型剥离，
// **没有 jsdom / 组件渲染**（技能 `celira-client-visual` §0 已登记）⇒ "接线还在不在"
// 只能按源码判。判错即红的样子：把 `catch (error) {…}` 改回 `catch {}`、把
// `const outcome = await downloadAttachment(...)` 改回 `void downloadAttachment(...)`、
// 或删掉那两条"说出口"（`role="alert"`）—— 任何一处，对应断言立刻红。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { apiErrorText, roomActionErrorText } from '../src/api/contractErrors.ts'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

/**
 * 词表命中 —— **不认"词里的一部分"**（与词表纪律同规：不用裸子串误报中立词汇）。
 *
 * 为什么需要它：`ProviderPanel.tsx` 里有一个 `formatRuntimeTime` 标识符，里面就有 `Runtime`
 * —— 裸子串扫会把它当成"上屏的内行词"（**假红**，而假红的下场是有人去放宽守卫）。
 * 判据因此是"命中处两侧都不是 ASCII 标识符字符"。
 */
function jargonHits(text: string, words: readonly string[]): string[] {
  const wordChar = (ch: string) => /[A-Za-z0-9_$]/.test(ch)
  return words.filter((word) => {
    for (let at = text.indexOf(word); at >= 0; at = text.indexOf(word, at + 1)) {
      if (!wordChar(text[at - 1] ?? '') && !wordChar(text[at + word.length] ?? '')) return true
    }
    return false
  })
}

const chatPage = read('../src/pages/ChatPage.tsx')
const fileRow = read('../src/components/celira/FileRow.tsx')
const lightbox = read('../src/components/celira/ImagePreviewLightbox.tsx')

/** 契约里"稳定分类 → 人话"的测试替身：Core 的错误对象只需带一个 `code`。 */
function coreError(code: string): Error {
  return Object.assign(new Error(`raw server body for ${code}`), { code })
}

// ── 一、撤回失败（ChatPage / handleRecall）───────────────────────────────

/**
 * ★ **进会话的落点 ＋ "加载更早"的两半 ＋ 行身份唯一**（2026-10-16 连续三刀，一次真机事故）。
 *
 * 判据（谁被改回去都会红）：
 * ① **消息列表必须是普通文档流** —— 不许再有虚拟列表（`useVirtualizer` / 绝对定位 + `translateY`）；
 *    真机实测"量出来的行高"从未生效（整屏按 84px 估值摆位 ⇒ 180 处重叠），
 *    专测探针 `scripts/home/probe-chat-layout.js` 就是为这一类立的；
 * ② **落点走纯函数**（`landingPlanFor`：有未读落分隔线、没有落最后一条），落法对 DOM 说；
 * ③ **翻页按高度差钉住**（不许退回"按索引落"）；
 * ④ **行身份唯一**（重复 id 只画一条并说出来）。
 */
test('★ 聊天时间线：普通文档流 ＋ 落点纯函数 ＋ 翻页钉位 ＋ 行身份唯一', () => {
  const chatCode = chatPage.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
  assert.ok(
    !/useVirtualizer|rowVirtualizer|translateY\(\$\{vi\.start\}/.test(chatCode),
    '★ 消息列表必须是**普通文档流**：虚拟列表那套"按量出来的高度摆位"在真机上从未生效（180 处重叠）',
  )
  assert.match(chatPage, /landingPlanFor\(\{/, '落点判据要在纯函数里（有离线用例）')
  assert.match(chatPage, /const settleLanding = /, '要有"头几帧重复落定"（行高在进房那一刻还会变）')
  assert.match(
    chatPage,
    /el\.scrollTop = before\.top \+ \(el\.scrollHeight - before\.height\)/,
    '★ 翻页落点按**高度差**钉住（绝对目标，逐帧纠偏）',
  )
  assert.ok(
    !/scrollToIndex\(prepended/.test(chatCode),
    '不许退回"按索引落"（前插那几行还没量过 ⇒ 会偏）',
  )
  assert.match(chatPage, /if \(seen\.has\(m\.id\)\) \{/, '重复 id 只渲染一条')
  assert.match(chatPage, /reportDuplicateRowId\(m\.id\)/, '重复 id 要说出来（不许只是"看着不清楚"）')
  assert.match(chatPage, /console\.error\(/, '那条报告要走 console.error（红路径不许静默）')
})

/**
 * ★ **"加载更早"的机制那半**（2026-10-16 主人真机反馈："…要么是显示位置有问题，不太明显那种，
 * 要么就是机制有问题？"）—— 机制在 store：**合并而不是替换**。
 *
 * 全仓**一处都不许**再有"把这一间的时间线整条换成某一页"的写法（2026-10-16 一共三处：
 * `refreshMessages`、开场时的缓存顶上、开场时的权威历史）—— 任何一处退回替换，
 * 人翻上来的历史都会被那一页顶掉。
 */
test('★ 加载更早（机制）：store 合并而不是替换 ＋ 游标不被顶掉 ＋ 乐观改名去重', () => {
  const store = read('../src/api/store.ts')
  assert.match(store, /mergeLatestPage\(/, '`refreshMessages` 必须走合并（历史不许被最新一页顶掉）')
  const replacements = [...store.matchAll(/messagesByRoom: \{ \.\.\.s\.messagesByRoom, \[roomId\]: \w+ \}/g)]
  assert.equal(
    replacements.length,
    0,
    `还有 ${replacements.length} 处"整条替换"：${replacements.map((m) => m[0]).join(' / ')}`,
  )
  assert.match(
    store,
    /s\.nextBeforeByRoom\[roomId\] === undefined \|\| s\.nextBeforeByRoom\[roomId\] === null/,
    '★ 翻页游标不许被某一页的游标顶掉（顶掉之后历史拉不回来）',
  )
  assert.match(store, /markMessagesSeen\(messages\)/, '翻回来的历史要登记为"见过"（不许再通知一遍）')
  assert.match(
    store,
    /settleOptimisticId\(s\.messagesByRoom\[roomId\] \?\? \[\], localId, serverId\)/,
    '★ 乐观改名要走去重那条纯函数（服务端回显先到时，直接改名会留下两条同 id）',
  )
})

/**
 * ★★ **"加载更早"曾经一次都没成功过**（2026-10-16 真机 + 专测探针一起定案）。
 *
 * 形状：store 里写成 `const loadOlderFn = api.loadOlder` 再 `loadOlderFn(roomId, before)`
 * —— 方法被**摘下来**之后 `this` 就丢了，`core.ts` 里那句 `this.api(...)` 抛
 * `TypeError: Cannot read properties of undefined (reading 'api')`，
 * 而那个 `catch { return 0 }` 把它**静默吞掉** ⇒ 界面上只是"点了没反应 / 滚到顶没动静"。
 *
 * 三条判据，缺一条这个 bug 就能原样回来：
 * ① 翻页必须**连着对象**调用（`api.loadOlder(...)`），不许摘成裸函数；
 * ② 失败**不许静默**（不许在 `loadOlder` 里 `catch { return 0 }`）；
 * ③ 页面上**说得出来**并给重试（顶部那行 `olderError` + 「重试」）。
 */
test('★★ 加载更早：方法不许摘下来调用 ＋ 失败不许静默 ＋ 页面要说得出来', () => {
  const store = read('../src/api/store.ts')
  assert.match(store, /await api\.loadOlder!?\(roomId, before\)/, '★ 必须连着对象调用（摘下来 `this` 就丢）')
  assert.ok(
    !/await loadOlderFn\(/.test(store),
    '★ 不许把 `api.loadOlder` 摘成裸函数再调用（`this` 会丢 ⇒ 抛错 ⇒ 从前被静默吞掉）',
  )
  // ② `loadOlder` 那一段里不许再出现"吞掉异常并返回 0"。
  const olderBody = store.slice(store.indexOf('loadOlder: async (roomId)'), store.indexOf('send: async (roomId'))
  assert.ok(
    !/catch\s*\{[^}]*return 0/.test(olderBody),
    '★ 翻页失败不许静默（`catch { return 0 }` 会把 TypeError 藏起来）',
  )
  assert.match(olderBody, /throw error/, '翻页失败要抛出去，由页面说人话 + 给重试')
  // ③ 页面那两半。
  assert.match(chatPage, /setOlderError\(roomActionErrorText\(error\)\)/, '页面要把失败说出来')
  assert.match(chatPage, /没能读到更早的消息/, '说人话，不许把原始 body 透出去')
  assert.match(chatPage, /正在读更早的消息…/, '读的时候顶部要有指示（主流形状：没有按钮，只有一个转圈）')
})

/**
 * ★ **"加载更早"必须是主流形状**（2026-10-16 主人："我看其他 im 好像没有这个选项吧。
 * 一般是自适应的那种？"）—— 判据三件：
 * ① **没有那颗按钮**（`加载更早消息` 这个字面量不许再出现在页面上）；
 * ② **滚到顶自动读**（触发在 `onTimelineScroll` 里，且 `scrollTop < 80` 那个判据在）；
 * ③ **滚不动也要读得到**（一屏装得下但还有更早 ⇒ 自动补页，有上限）。
 *
 * ⚠ 另一半（真的读到没有）不在源码里：那是专测探针 `probe-chat-layout.js` 的判据
 * （它量"滚到顶之后行数变多"）。
 */
test('★ 加载更早：主流形状（无按钮 / 滚到顶自动读 / 滚不动也读得到）', () => {
  // ⚠ 判据只看**代码**：注释里为了讲清这次改动会提到那颗按钮的名字，剔掉注释再判
  //   （否则守卫会骂自己 —— 这个坑本文件已经踩过一次）。
  const chatCode = chatPage.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
  assert.ok(
    !chatCode.includes('加载更早消息'),
    '★ 时间线上不许再有「加载更早消息」那颗按钮（主流 IM 没有这个形状）',
  )
  assert.match(chatPage, /if \(!vp \|\| vp\.scrollTop >= 80\) return/, '滚到顶（<80px）就触发')
  assert.match(chatPage, /const AUTO_FILL_MAX_PAGES = \d+/, '滚不动那一档要有上限（别把世界问烦）')
  assert.match(
    chatPage,
    /vp\.scrollHeight <= vp\.clientHeight \+ 8/,
    '★ 列表不可滚 ＋ 还有更早 ⇒ 自动补一页（否则更早的消息没有入口）',
  )
  assert.match(chatPage, /timelineViewport\(\)/, '★ 视口要**现找**（Radix 会在 React 背后换掉那个节点）')
})

test('撤回失败的两种稳定分类各有**逐字**的人话（超窗 / 无权）', () => {
  assert.equal(
    roomActionErrorText(coreError('message_recall_window_expired')),
    '已超过可撤回时间（发送后 2 分钟内）',
  )
  assert.equal(roomActionErrorText(coreError('message_not_found')), '消息不存在或无权撤回')
  assert.match(roomActionErrorText(coreError('message_not_found')), /无权/)
  // 未知分类也得说人话（不许把原始 body 透出去）
  const unknown = roomActionErrorText(new Error('raw server body: sk-secret'))
  assert.ok(unknown.length > 2, '未知分类要有通用文案')
  assert.ok(!unknown.includes('sk-secret'), '绝不回显原始错误正文')
  assert.ok(!unknown.includes('raw server body'), '绝不回显原始错误正文')
})

test('`handleRecall` 的 catch 用上了映射（不许再静默吞）', () => {
  const start = chatPage.indexOf('const handleRecall = useCallback(')
  assert.ok(start > 0, '找不到 handleRecall —— 改名了？这条守卫要跟着改')
  const block = chatPage.slice(start, start + 900)
  const catchBlock = /catch\s*\([^)]*\)\s*\{([^}]*)\}/s.exec(block)
  assert.ok(catchBlock, 'handleRecall 里没了 catch —— 那意味着失败又回到静默')
  assert.match(
    catchBlock[1],
    /roomActionErrorText\(/,
    'catch 里必须把稳定分类换成那句人话（改回 `catch {}` 就是静默，判错即红）',
  )
})

test('撤回失败文案真的渲染出来了（设了状态不显示等于没修）', () => {
  assert.match(
    chatPage,
    /\{actionError\s*&&\s*\(/,
    'ChatPage 里要有 `{actionError && (…)}` 那条提示（形状照 MessageInput 的 role="alert"）',
  )
  assert.match(chatPage, /role="alert"/, '提示要能被读屏念出来（role="alert"）')
})

// ── 二、附件下载失败（FileRow / ImagePreviewLightbox）────────────────────

test('下载失败的分类落到"附件下载失败"——**不是**通用兜底', () => {
  // `fetchBlob` 抛的是 `attachment_download_<status>`（`client.ts` 的 codePrefix + 状态码）。
  // 不折这一族的话会落进 default —— 而通用兜底曾经写的是「发送失败，请重试」，
  // 那等于**把下载失败说成发送失败**（说反了）。兜底现已改成中性文案，但判据照旧：
  for (const code of ['attachment_download_401', 'attachment_download_404']) {
    // **正向**判据就够：断言它等于"附件下载失败"那句人话，等价地排除了"说成别的"。
    // ⚠ 原先这里还有一条 `notEqual(…, '发送失败，请重试')`：兜底改中性之后它会**恒真**，
    // 变成一条永远绿的假判据 ⇒ 已删（本仓"别断『不存在某串』"那条纪律的又一例）。
    assert.equal(apiErrorText(code), '附件下载失败，请重试', code)
  }
  assert.equal(apiErrorText('attachment_download_failed'), '附件下载失败，请重试')
})

test('⚠ 上传**没有** `attachment_upload_*` 那一族（原判据断言的是一个幽灵码）', () => {
  // 原注释写「上传那一族同理（`uploadMultipart` 抛 `attachment_upload_<status>`）」——
  // **那句是错的**：`uploadMultipart` 没有 `codePrefix` 参数（只有 `fetchBlob` 有），
  // 它抛的是 im 信封里的真码 ⇒ 上传失败走真词映射，前缀那一族根本不存在。
  // 这一条同时钉住"真词的文案在位"与"幽灵不许回来"。
  assert.equal(apiErrorText('invalid_attachment_metadata'), '附件信息异常，请重新上传')
  assert.equal(apiErrorText('payload_too_large'), '内容过长，超过了服务端上限')
  const cases = [...read('../src/api/contractErrors.ts').matchAll(/case '([^']+)'/g)].map(
    (m) => m[1],
  )
  assert.ok(
    !cases.some((code) => code.startsWith('attachment_upload')),
    '`attachment_upload*` 没有生产者，不许再出现在映射表里',
  )
})

test('`downloadAttachment` 的失败分支**返回人话**，不再有 `return false` 那种吞法', () => {
  // ⚠ 为什么是源级判据：`lib/attachmentDownload.ts` 用了 `@/` 别名（`@/api/store` 的 `getApi`），
  // 而 `node --test` **不解析 vite 别名** ⇒ 这个文件在测试里 import 不进来（实测
  // `ERR_MODULE_NOT_FOUND: Cannot find package '@/api'`）。所以按源码判 —— 与撤回那两条同一档。
  const source = read('../src/lib/attachmentDownload.ts')
  assert.ok(
    !/return false/.test(source),
    '不许再出现 `return false`（那正是"调用点拿到布尔、随手 void 掉"的源头）',
  )
  assert.match(
    source,
    /catch\s*\(error\)\s*\{[\s\S]{0,400}return \{\s*ok:\s*false,\s*text:\s*errorText\(error\)\s*\}/,
    'catch 里要把稳定分类换成那句人话（`errorText`），并且**包在收场里返回**',
  )
  assert.match(
    source,
    /return \{\s*ok:\s*false,\s*text:\s*apiErrorText\('attachment_download_failed'\)\s*\}/,
    '没有任何可用下载路径时也要说一句，不假装成功',
  )
  // ⚠ **形状 2026-10-08 变过**（壳里拿得到真实结果之后，"成功"分成了两档：浏览器里不说、
  //   壳里要说且说得出落点）⇒ 从 `Promise<string | null>` 变成判别式。**判据的意图一字没变**：
  //   返回值必须是"该说的话"的载体，不是布尔、不是被丢掉的空。
  assert.match(source, /Promise<DownloadOutcome>/, '返回值形状 = 收场（`ok` 分两档）')
  assert.match(
    source,
    /from '@\/lib\/downloadOutcome'/,
    '收场的判据只有一处（`lib/downloadOutcome.ts`），这里只许引用',
  )
  assert.ok(
    !/function downloadOutcomeText/.test(source),
    '收场的判据只有一处：这里不许就地再写一遍 `downloadOutcomeText`',
  )
})

test('两个下载入口都**消费了返回值**并显示出来（原先都是 `void …`）', () => {
  for (const [name, source] of [
    ['FileRow.tsx', fileRow],
    ['ImagePreviewLightbox.tsx', lightbox],
  ] as const) {
    assert.match(
      source,
      /const outcome = await downloadAttachment\(/,
      `${name}: 必须消费 downloadAttachment 的返回值（改回 \`void downloadAttachment(...)\` 就是静默）`,
    )
    assert.match(
      source,
      /downloadOutcomeText\(outcome\)/,
      `${name}: 收场要变成一句能显示的话（判据在 lib/downloadOutcome.ts，组件不自己判）`,
    )
    assert.ok(
      !/void downloadAttachment\(/.test(source),
      `${name}: 不许再出现被丢弃的 \`void downloadAttachment(...)\``,
    )
    assert.match(
      source,
      /role="alert"|role=\{[^}]*'alert'[^}]*\}/,
      `${name}: 失败要说出口（\`role="alert"\`；成功那句可以是 \`role="status"\`）`,
    )
  }
})

// ── 三、语音附件的播放失败（AudioAttachment）──────────────────────────────

test('点播放失败要有出口（原先 `.catch(() => {})` 吞掉）', () => {
  const source = read('../src/components/celira/AudioAttachment.tsx')
  assert.ok(
    !/audio\.play\(\)\.catch\(\(\) => \{\}\)/.test(source),
    '不许再出现空回调吞掉播放失败（那是"点了没反应"）',
  )
  assert.match(
    source,
    /audio\.play\(\)\.catch\(\(\) => setPlayFailed\(true\)\)/,
    '播放失败要落到一个可见状态上',
  )
  assert.match(source, /role="alert"/, '失败要说出口（role="alert"）')
})

test('死模块 `lib/ringtone.ts` 保持已删（0 消费者 ⇒ 按纪律删，不是"忘了接线"）', () => {
  // 这条把"删掉"钉成读数：谁真要把它接回来（接线给门铃/通知），得**有意识地**改这里。
  // 来历可考：`d9967ef`（双系统目录重排）；删的依据：全仓引用 = 0（`outputs/SILENT_RED_PATH_SURVEY_*.md`）。
  assert.throws(() => read('../src/lib/ringtone.ts'), /ENOENT/)
})

// ── 四、条件事实不许退化成常驻讲解（文案减重刀 / 称谓刀 2026-09-23）──────
//
// 模型配置页那句「这个数字还是上一个模型的，清空再保存会自动重问」是**条件事实**：
// 只在「改了模型名、但窗口还是旧模型留下的那个数」这一态才说。若把它改成常驻写上屏，
// 就退化成"安静时刻讲机制"（正是本刀要治的病）。
//
// ⚠ 也是**源级守卫**（无 jsdom）：判"它是不是条件渲染"，只能按源码判。判错即红的样子：
// 删掉 `{staleWindow && (…)}` 的守卫、或把条件去掉直接渲染那行字。

test('模型配置页的「窗口是上一个模型的」是**条件**事实（`staleWindow` 守卫 + 依赖旧模型名）', () => {
  // ⚠ **2026-10-12 第三刀**：`staleWindow` 那一段从页面搬进了 `ProviderPanel.tsx`
  //   （那一处只说"某一家"）。判据一字未改，只是它现在住在哪。
  const page = read('../src/components/celira/ProviderPanel.tsx')

  // ① 判据存在且**真的有条件**：新模型名 ≠ 旧模型名、且窗口非空。
  assert.match(
    page,
    /const staleWindow\s*=\s*[\s\S]{0,220}?editingEntry !== null[\s\S]{0,220}?draft\.model !== editingEntry\.model[\s\S]{0,220}?draft\.contextWindow\.trim\(\) !== ''/,
    'staleWindow 的判据必须同时含「在编辑」「模型名变了」「窗口非空」—— 少一个就会出现假阳性提示',
  )

  // ② 那句话被这个判据**守着**渲染（不是常驻）。
  assert.match(
    page,
    /\{staleWindow\s*&&\s*\(/,
    '那句事实必须包在 `{staleWindow && (…)}` 里 —— 退成常驻就是"安静时刻讲机制"',
  )

  // ③ 负对照：常驻渲染那种写法（直接摆一句「上一个模型」而不带 staleWindow）要被认出来。
  const resident = '这个数字还是上一个模型的，清空再保存会自动重问。'
  assert.ok(page.includes(resident), '那句话本身要还在（否则这条守卫守的是空气）')
  const guardedShape = /\{staleWindow\s*&&\s*\(\s*<>?\s*[^)]*上一个模型/.test(page)
  assert.ok(
    guardedShape || /\{staleWindow\s*&&[\s\S]{0,120}?上一个模型/.test(page),
    '那句话必须紧跟 staleWindow 守卫出现（负对照：脱离守卫的常驻写法会红）',
  )
})

// ── 五、「这个模型能看图」那一格：**静默关不掉**是一条红路径（2026-10-17）──────
//
// 症状的形状与上面几件同族：**没有任何报错**。用户取消勾选 ⇒ 页面发出去的体里少了那一格
// ⇒ 门把它读成"没说"（三态，见 `api/homeConfig.ts` 的 `declarationBody`）⇒ 从旧条目**原样
// 带回来** ⇒ 这一格**永远关不掉**，而界面照旧说"已保存"。
//
// 判据四条，缺一条都不算接好：
//  ① **打开已有条目时按 `entry.vision ?? false` 初始化**（表单总是"说了"，用户才碰得到"关掉"）；
//  ② **勾选框真的接在 `onVision` 上**（摆一个没人接的控件 = 静默）；
//  ③ **勾的状态读草稿那一格**（写死 `false` 就是"已经开着的那一条显示成关着"）；
//  ④ **不勾的后果写在勾旁边**（本仓纪律「人不许比系统知道得更少」：说清哪一条拒绝、以及
//     这是**这台机器**的事实而不是某一支自己的设置）。
//
// ⚠ 源级守卫（与本节其余各条同一个理由）：这个仓库的 client 测试是 `node --test` + TS 类型
// 剥离，**没有 jsdom / 组件渲染** ⇒ "接线还在不在"只能按源码判。类型系统只挡得住一半：
// `HomeConfigDraft.vision` 是**可选**的，所以 `draftOf` 漏掉它、或 `checked` 写死 `false`，
// `tsc` 一声不吭。

test('★ 「这个模型能看图」：打开已有条目按 `entry.vision ?? false` 初始化，且勾真的接了线', () => {
  const page = read('../src/pages/ModelProvidersPage.tsx')
  const panel = read('../src/components/celira/ProviderPanel.tsx')

  /** ① 打开一条已有的 ⇒ 草稿那一格按**这一条自己说的**初始化（读不到按"不支持"，与门同一条判据）。 */
  const draftInit = /vision:\s*entry\.vision\s*\?\?\s*false/
  /** ② 页面把那一格接到草稿上（`false` 也要写进草稿：它就是"说了不支持"）。 */
  const pageWiring = /onVision=\{\(?(?:vision|value|v)\)?\s*=>[\s\S]{0,80}?vision\s*\}/
  /** ③ 勾的状态读草稿、动作调 `onVision`（两半都要：只画不接就是静默）。 */
  const checked = /checked=\{draft\.vision === true\}/
  const onChange = /onCheckedChange=\{[^}]*onVision\(/

  assert.match(
    page,
    draftInit,
    '打开一条已有的，勾必须按 `entry.vision ?? false` 初始化 —— 少了它，用户碰不到"明确关掉"这一态',
  )
  assert.match(page, pageWiring, '页面要把 `onVision` 接到草稿的 `vision` 那一格上')
  assert.match(
    panel,
    checked,
    '勾的状态要读草稿那一格（`undefined` 按不支持显示，与门同一条判据）',
  )
  assert.match(
    panel,
    onChange,
    '勾选框要真的调 `onVision` —— 摆一个没人接的控件，用户点了没反应而界面不报错',
  )

  // 负对照（守卫不是空转）：三种"静默关不掉"的写法各自要被认出来。
  assert.ok(!draftInit.test('    vision: entry.vision,'), '漏了 `?? false` ⇒ 老门那一档显示成"没说"')
  assert.ok(
    !pageWiring.test('onVision={(vision) => setDraft((c) => ({ ...c }))}'),
    '接了 prop 却不写进草稿 ⇒ 勾了等于没勾',
  )
  assert.ok(!checked.test('checked={false}'), '写死 `false` ⇒ 已经开着的那一条显示成关着')
  assert.ok(!onChange.test('onCheckedChange={() => {}}'), '空回调 ⇒ 点了没反应，且不报错')

  // ④ 后果写在勾旁边（钉的是**事实**不是整句，与 `page-subject-lines.test.ts` 同一风格）。
  assert.ok(
    panel.includes('unsupported-content-type'),
    '不勾的后果要写在勾旁边：说清图片会被哪一条拒绝',
  )
  assert.ok(panel.includes('这台机器'), '要说清这是**这台机器**的事实，不是某一支自己的设置')
})

// ── 六、换型号：这一格**随模型走**，不许静默沿用（2026-10-17）────────────────
//
// 同一个病的第二个门：`vision` 是**这个模型**的属性，而它住在**这一条钥匙**的表单里。
// 两条换型号的路各缺一半：
//  · **模型目录那条**（`useModel`）**不走草稿**（它自己拼请求体）⇒ 旧值被门"从旧条目原样
//    带过来"，而界面还摆着**旧型号名**（下一次点保存就把刚换的型号写回去）；
//  · **手填型号名那条**走草稿（所见即所存，对），缺的只是"这一格随模型走"这件**知识**。
//
// 判据因此分两处，各自贴着那条路的形状：目录那条**改数据**（显式复位 ＋ 拨回表单），
// 手填那条**只补知识**（条件提示，与 `staleWindow` 同形）。
// ⚠ 复位的判据落在纯函数 `api/homeConfig.ts` 的 `modelChangeDraft`（**有离线用例**，
// 见 `tests/home-config.test.ts`）；这一条只钉"接线还在不在"（无 jsdom，只能按源码判）。

test('★ 换型号那两条路：目录那条显式复位并拨回表单，手填那条给条件提示', () => {
  const page = read('../src/pages/ModelProvidersPage.tsx')
  const panel = read('../src/components/celira/ProviderPanel.tsx')

  // ① 目录那条：走纯函数（判据有离线用例）＋ 写完之后把那几格**拨回表单**。
  const start = page.indexOf('const useModel = async')
  const end = page.indexOf('const field = (key: HomeConfigTextField)')
  assert.ok(
    start > 0 && end > start,
    '这条守卫要能找到 `useModel` 那一段 —— 找不到就是源码结构变了，回来改它（不许默默变绿）',
  )
  const modelChange = page.slice(start, end)
  assert.match(modelChange, /modelChangeDraft\(/, '换型号那一次要走纯函数（判据在那儿）')
  assert.match(
    modelChange,
    /setDraft\(/,
    '写完之后要把那几格拨回表单 —— 否则表单还摆着旧型号名，下一次保存把刚换的型号写回去',
  )
  assert.match(modelChange, /vision:/, '拨回的那几格里要有「能不能看图」这一格')

  // ② 手填那条：**条件**提示（不是常驻讲解），且只在"这一格正说能看图"时说。
  assert.match(
    panel,
    /const staleVision\s*=\s*[\s\S]{0,200}?editingEntry !== null[\s\S]{0,200}?draft\.model !== editingEntry\.model[\s\S]{0,160}?draft\.vision === true/,
    '判据必须同时含「在编辑」「型号名变了」「这一格正说能看图」—— 少一个就会出现假阳性提示',
  )
  assert.match(panel, /\{staleVision\s*&&\s*\(/, '那句提示要被判据守着 —— 退成常驻就是"安静时刻讲机制"')
  assert.ok(
    panel.includes('这个勾还是上一个模型的答案'),
    '那句话本身要还在（否则这条守卫守的是空气）',
  )
})

// ── 七、面板里那几句也不许有内行词（**sweep 到不了它**，2026-10-17）────────────
//
// 为什么需要这一条：内行词表本来就有执行者 —— `probe.mjs` 的 `sweep` 随 `npm run verify`
// 逐条路由扫**渲染后的正文**。但它只看**默认渲染**，而这一页在默认渲染下**画不出那个面板**：
// 它读的是**那台机器**的模型配置门，门没开时页面上是"读不到配置"那张卡（实测 29 条路由那
// 一次就是这样，5191 上没有门在听）⇒ 面板里的每一句在 sweep 眼里**都不存在**。
//
// ⚠ 判据与 `tests/user-theme.test.ts` 那一族同一条（**同一份词表**，判据落在源码上）：
// 不是替换 sweep，是补它的盲区。词表**直接取权威那份**（抄回去就是两个权威，一定会漂）。
//
// ⚠ **不是唯一一道**：`npm run verify:ui` 那一档**到得了**这一页 —— 门开着时它把**整页正文**
// 拿去做人话自检（2026-10-17 实测 128/128 通过）。这一条补的是**不需要真机、不需要门**的那一档：
// `npm run verify` 里的 sweep 永远看不见面板，所以它拦不住"面板里写了个内行词"。

test('★ 模型配置面板的文案不许有内行词（门没开时这一页只画"读不到"，sweep 看不见面板）', () => {
  const { JARGON } = createRequire(import.meta.url)('../scripts/jargon.mjs') as { JARGON: string[] }
  const source = read('../src/components/celira/ProviderPanel.tsx')
  // 注释里照旧合法（`DESIGN_SYSTEM.md` 明说那是给开发者读的）⇒ **先摘注释再扫**。
  const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

  const leaked = jargonHits(code, JARGON)
  assert.deepEqual(leaked, [], `面板里出现了内行词：${leaked.join(' / ')}`)

  // 负对照（守卫不是空转）：真上屏的内行词要被抓住，标识符里那一段不许误报。
  assert.deepEqual(
    jargonHits('<div>本机受管 Runtime</div>', JARGON),
    ['Runtime'],
    '真上屏的内行词必须被抓住',
  )
  assert.deepEqual(
    jargonHits('import { formatRuntimeTime } from "@/lib/time"', JARGON),
    [],
    '标识符里那一段不许误报（假红会让人去放宽守卫）',
  )
})
