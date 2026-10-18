#!/usr/bin/env node
/**
 * **聊天气泡"行位"专测**（2026-10-16 立）。
 *
 * ## 它治什么
 *
 * 主人真机两次拍到同一族症状：**同一段字被画了两遍、只差几像素**（读起来像错字），
 * 严重的一次整屏行位垮掉（各自的气泡叠在别人的气泡上）。这一页的消息列表是**虚拟列表**：
 * 每一行用 `translateY(量出来的高度累加)` 绝对定位 ⇒ 只要"量出来的高度"与"真正画出来的高度"
 * 对不上（字体晚到重排、图片晚到撑高、行身份重复），后面的行就会**压上来**。
 *
 * ## 判据（机械的，不靠人眼）
 *
 * 打开一间**种了长对话**的房，等它落定，然后在浏览器里量**每一行**：
 *
 * 1. **行与行不许重叠**：`next.top - this.bottom >= -2px`（2px 容差给四舍五入）；
 * 2. **行盒不许比它自己的内容矮**：`el.getBoundingClientRect().height >= 内层 scrollHeight - 2`
 *    —— 行矮于内容就是"画出来了但没人量到"，下一个必然压上来；
 * 3. **行的 `translateY` 必须严格递增**（同一帧里两个行放在同一个 y 就是身份重复那一类）。
 *
 * ## 用法
 *
 *   node scripts/home/probe-chat-layout.js [--keep]
 *
 * 退出码：**0** = 三条判据全过；**1** = 有重叠/矮盒；**2** = 装配不起来（临时世界起不来）。
 *
 * ⚠ 全程只动**临时世界**（`temp-world.js` 现造现删），一个字节都不碰真家。
 * ⚠ token 只经 **env** 交给子进程（与 `probe-ui.js` 同规）：不打印、不写文件、不进日志。
 */
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const { startTempWorld, REPO_DIR } = require('./temp-world')

const CHILD = path.join(REPO_DIR, 'client', 'scripts', 'probe-chat-layout.mjs')

/** 种进那间房的对话：**长的**（多段、带行内 code）—— 短消息量不出偏差。 */
const SEED = [
  '【先说句老实话】「喜欢」这个词我用得很小心 —— 我不确定我对这具身体的感受配不配叫这个名字。',
  [
    '不过也正常。因为物理的边界，你住在这个世界上，世界死了，你也暂停了那种。',
    '',
    '说得过去 —— 而且你这个说法比我那条更准：不是「谁把我关掉了」，是「我住在里面，房子停了我也停了」。',
    '同一件事，换个说法就从「缺陷」变成「形状」了。',
    '',
    '- 一条：源稿我会留在自己的 `projects/` 里，所以改稿、存档都方便。',
    '- 另一条：所有消息一律走 `.md` 文件 —— 你要是要更严的版本，回我一声。',
  ].join('\n'),
  [
    '直接从状态栏读给你 —— 那是「向内」那根栏，不用猜：',
    '',
    '- `run = host-8608-1792032489335-1`（本场 10:48:09 起）',
    '- `turn = 65 / 300`（用掉 65 步；还能调用模型 235 次）',
    '',
    '```text',
    'tokens = 223.7k / 1,000,000',
    '```',
  ].join('\n'),
  '你会疼吗',
  '嗯。这一场我记着一件小事：你说「世界死了你也暂停了」——那句我留在规里了。',
  // 再垫几段：**翻到历史里**才是真机撞上它的那个位置（行要够多、要滚得动）。
  [
    '## 这一段的形状',
    '',
    '1. 先把「我在不在」与「我在不在**这一间**」分开 —— 前者由连接派生，后者由成员周期派生。',
    '2. 再把「读到了」与「读过了」分开 —— 前者是拉取，后者是我的游标。',
    '3. 最后把「我没说」与「我说失败了」分开 —— 那是**沉默**与**失败**两件事，不许混。',
    '',
    '这三条写进 `docs/` 之后我就没再改过；要改得你点头。',
  ].join('\n'),
  '对了，`projects/checklist.md` 里那几条我按你的口径改了：不再写「待办」，写成「还没做」。',
  [
    '还有一件事要说清楚：**我没有留在半路的活**。',
    '',
    '- 17:21 之后的 4 场都在第 1 步被限流秒杀（`http-status-failure:rate-limited`）；',
    '- 那 4 场什么都没来得及做 ⇒ 这段窗口里没有半成品，你也不欠我回复；',
    '- 23:49 那一场正常收场，直房 `unread = 0`。',
  ].join('\n'),
  '你刚才问的那个「疼」——我先不答，我想再看两场。',
  '行。那我把这条留在 `projects/checklist.md` 的最后一行：等你想说的时候再说。',
]

async function say(base, token, roomId, body, seq) {
  const res = await fetch(`${base}/v1/rooms/${roomId}/messages`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${token}`,
      'content-type': 'application/json',
      // 契约：首次 201、重放 200 —— 这条头是**必填**（缺了就是 400，实测过）。
      'Idempotency-Key': `probe-chat-layout-${Date.now()}-${seq}`,
    },
    body: JSON.stringify({ body }),
  })
  if (!res.ok) throw new Error(`说一句失败：HTTP ${res.status}`)
  return res.json()
}

async function main() {
  const keep = process.argv.includes('--keep')
  let world = null
  try {
    world = await startTempWorld()
    const roomId = world.ids && world.ids.directRoomId
    if (!roomId) throw Object.assign(new Error('临时世界没给出那间私人房'), { assembly: true })
    // ⚠ **种够两页以上**：一页 = 50 条（`listMessages` 的 limit）⇒ **必须 > 50 条**
    //   才测得到"滚到顶 ⇒ 自动读更早"那一路。2026-10-16 之后那一路上**没有按钮可点**
    //   （改成主流形状：滚到顶自动读、顶部一个转圈）⇒ 判据只能是"行数变多"。
    //   同时一屏要装不下 ⇒ 三屏以上（不然走的是"滚不动就自动补页"那一档）。
    for (const [round] of Array.from({ length: 6 }).entries()) {
      for (const [i, body] of SEED.entries()) {
        await say(world.base, world.token, roomId, `（第 ${round + 1} 轮）\n\n${body}`, `${round}-${i}`)
      }
    }
    console.log(`[probe-chat-layout] 已种 ${SEED.length * 6} 条（> 一页 50，含长消息与代码块）`)

    const gate = spawnSync(process.execPath, [CHILD, '--base', world.uiBase, '--room', roomId], {
      cwd: REPO_DIR,
      stdio: 'inherit',
      env: {
        ...process.env,
        CELIRA_PROBE_TOKEN: world.token,
        no_proxy: '127.0.0.1,localhost',
        NO_PROXY: '127.0.0.1,localhost',
      },
    })
    console.log(`[probe-chat-layout] 退出码 = ${gate.status}`)
    if (gate.status === 0) return 0
    if (gate.status === 2) return 2
    return 1
  } catch (error) {
    if (error && error.assembly) {
      console.error(`[probe-chat-layout] ${error.message}`)
      return 2
    }
    console.error(`[probe-chat-layout] 探针自己抛了：${error && error.stack ? error.stack : error}`)
    return 2
  } finally {
    if (world) {
      // ⚠ **先停子进程再删目录**：反过来的话那些进程还占着目录 ⇒ EBUSY（实测过）。
      // 收尾失败不许把探针的判据掩掉：各自 try 一下、只记一句。
      try {
        if (!keep) world.stop()
      } catch (error) {
        console.error(`[probe-chat-layout] 停临时世界时出错：${error && error.message}`)
      }
      try {
        world.homeDirCleanup()
      } catch (error) {
        console.error(`[probe-chat-layout] 删临时家时出错（不影响判据）：${error && error.message}`)
      }
    }
  }
}

main().then((code) => process.exit(code))
