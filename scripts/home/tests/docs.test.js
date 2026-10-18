'use strict'
// 文档与代码对账（机械判据）：手册教的那条命令必须真的存在，退出码的数字必须与代码一致。
//
// 为什么值得有这几条：这是"发了包的手册"与"跑起来的代码"之间最容易悄悄分叉的地方
// （TASK_48 的教训：手册整节教用户跑一个没随包发出去的 exe）。判据要机械，不要靠读。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const { EXIT, UNSETTLED } = require('../codes')
const P = require('../paths')

let cases = 0
function docCase(name, fn) {
  test(name, () => {
    fn()
    cases += 1
  })
}

const readme = fs.readFileSync(path.join(P.REPO, 'scripts', 'HOME_MANUAL.md'), 'utf8')

docCase('入口存在：仓库根有 home.js', () => {
  assert.ok(fs.existsSync(path.join(P.REPO, 'home.js')))
})

docCase('手册的三条命令讲的是 node home.js（不再教 bash 那三个脚本）', () => {
  assert.ok(readme.includes('`node home.js up`'), '手册里没有 node home.js up')
  assert.ok(readme.includes('`node home.js status`'), '手册里没有 node home.js status')
  assert.ok(readme.includes('`node home.js down`'), '手册里没有 node home.js down')
  for (const stale of ['bash scripts/home_up.sh', 'bash scripts/home_status.sh', 'bash scripts/home_down.sh']) {
    assert.ok(!readme.includes(stale), `手册里还留着过时的命令：${stale}`)
  }
})

docCase('退出码表在手册里，且数字与 codes.js 一致', () => {
  assert.ok(readme.includes('退出码一张表'))
  for (const row of ['| 0 |', '| 1 |', '| 2 |', '| 3 |', '| 4 |']) {
    assert.ok(readme.includes(row), `退出码表里缺这一行：${row}`)
  }
  assert.equal(EXIT.OK, 0)
  assert.equal(EXIT.USAGE, 1)
  assert.equal(EXIT.ENV_UNAVAILABLE, 2)
  assert.equal(EXIT.NEEDS_YOU, 3)
  assert.equal(EXIT.PARTIAL, 4)
  // `agent_run` 那一行写着 9 = 停在等待（环境里留下一个未收尾的活动）。
  assert.equal(UNSETTLED, 9)
  assert.ok(/`9`\s*停在等待/.test(readme), '手册里没有 agent_run 的 9 = 停在等待')
})

docCase('两份根 README 的快速开始也改成了 node home.js', () => {
  const en = fs.readFileSync(path.join(P.REPO, 'README.md'), 'utf8')
  const zh = fs.readFileSync(path.join(P.REPO, 'README.zh-CN.md'), 'utf8')
  assert.ok(en.includes('node home.js up'), 'README.md 没跟上')
  assert.ok(zh.includes('node home.js up'), 'README.zh-CN.md 没跟上')
})

docCase('三个 .sh 已退役（TASK_51 删掉；迁移收尾的标志，不是"还在存档"）', () => {
  for (const name of ['home_up.sh', 'home_down.sh', 'home_status.sh']) {
    assert.ok(!fs.existsSync(path.join(P.REPO, 'scripts', name)), `${name} 应当已经删掉`)
  }
  // 打包脚本也不能再拷它们（否则发布时会因为源文件不存在而失败）。
  const packaging = fs.readFileSync(path.join(P.REPO, 'scripts', 'make_release.ps1'), 'utf8')
  for (const name of ['home_up.sh', 'home_down.sh', 'home_status.sh']) {
    assert.ok(!packaging.includes(`'${name}'`), `make_release.ps1 还在拷 ${name}`)
  }
})

docCase('凭据纪律自查：源码里不许出现**像凭据的字面量**（只有变量名允许）', () => {
  // 判据要机械、且不能误报：变量名字面量（`CELIRA_IM_BACKEND_AUTH`）是**要求**出现的，
  // 所以查的是"凭据的形状"——token 前缀 / Bearer 串 / 长随机串。
  const SECRET_SHAPE =
    /(clr_[A-Za-z0-9_-]{8,})|(sk-[A-Za-z0-9_-]{8,})|(Bearer\s+[A-Za-z0-9._-]{12,})|([A-Za-z0-9]{40,})/
  const files = [path.join(P.REPO, 'home.js')]
  for (const name of fs.readdirSync(path.join(P.REPO, 'scripts', 'home'))) {
    if (name.endsWith('.js')) files.push(path.join(P.REPO, 'scripts', 'home', name))
  }
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8')
    for (const line of text.split('\n')) {
      const hit = SECRET_SHAPE.exec(line)
      assert.ok(!hit, `${path.relative(P.REPO, file)} 里像是放了凭据原文：${line.trim()}`)
    }
  }
})

docCase('凭据纪律自查：两份凭据都是**按变量名**读的（正向对照）', () => {
  const up = fs.readFileSync(path.join(P.REPO, 'scripts', 'home', 'up.js'), 'utf8')
  const provision = fs.readFileSync(path.join(P.REPO, 'scripts', 'home', 'provision.js'), 'utf8')
  assert.ok(up.includes("readUserEnv('DEEPSEEK_API_KEY')"), 'up 没有按变量名读 DEEPSEEK_API_KEY')
  assert.ok(provision.includes("const BACKEND_ENV = 'CELIRA_IM_BACKEND_AUTH'"), 'provision 没认后端凭据变量名')
  // 原约束（2026-09-25 之前）：后端凭据必须经 `writeUserEnv(BACKEND_ENV)` 写回**用户环境变量**。
  // 新边界（2026-10-01 起）：那份事实的作用域是「**Agent × 那个世界**」⇒ 权威位置是
  // **按「世界 × 身份」分条的本地存储**（`credentials.js`，住在**家之外**的机器级目录）；
  // 环境变量降级为**只读**的老路/手工路。
  // 真实消费者：`provision.js`（写）与 `up.js` / `down.js` / `status.js`（读）。
  // 越界反例（这几条断言就是抓它们的）：把凭据写回环境变量（机器级单槽，两个家会抢）、
  // 只按身份分条（同一个 agent id 在两个世界里会撞同一个文件），
  // 或者把取值口径绕过 `credentials.resolve` 自己读一份。
  assert.ok(
    provision.includes('credentials.write({ world: base, agentId }'),
    '后端凭据不是经「世界 × 身份」分条的本地存储写回的',
  )
  assert.ok(!provision.includes('writeUserEnv'), '凭据不许再写回用户环境变量（那是机器级单槽）')
  // ⚠ 断言**不许依赖换行与缩进**（2026-10-03 修）：从前这里写的是
  // `provision.includes('credentials.resolve(\n    { world: base, agentId }')`——
  // 在 CRLF 检出（Windows 默认）上 `\n` 匹配不到 `\r\n`，这条自查**一直是红的**，
  // 而它守的正是"凭据取值必须带世界那一维"这条真约束（陈旧的门禁比没有门禁更坏：
  // 真回归会被淹没在噪音里）。约束不变，判据改成空白无关。
  assert.ok(
    /credentials\.resolve\(\s*\{\s*world:\s*base,\s*agentId\s*\}/.test(provision),
    '凭据的取值口径必须走 credentials.resolve，且带世界那一维',
  )
  // 读的那一侧同理（起宿主 / 关机归属证明 / 状态读数）：键都要带世界。
  for (const file of ['up.js', 'down.js', 'status.js']) {
    const text = fs.readFileSync(path.join(P.REPO, 'scripts', 'home', file), 'utf8')
    assert.ok(
      /credentials\.resolve\(\s*\{[^)]{0,160}?agentId/.test(text),
      `${file} 里的凭据读取必须带「世界 × 身份」（不然两支在不同世界会撞车）`,
    )
  }
})

docCase('手册写清了「布置键按家取」与老家的后果（2026-10-01）', () => {
  // 用户可见行为变了：一个世界住多个家（键按家取）；老家（没有 home_id）第一次跑会另建一支 Agent。
  // 判据机械：这两条事实必须在手册里能被读到（改错字可以，事实不许消失）。
  assert.ok(readme.includes('home_id'), '手册里没有 home_id（家 id）')
  assert.ok(readme.includes('按家取'), '手册里没有「布置键按家取」')
  assert.ok(readme.includes('另建一支 Agent'), '手册没有写清老家的后果（会另建一支 Agent）')
  assert.ok(readme.includes('旧那支留着不动'), '手册没有写清旧那支的去向（留着不动）')
})

docCase('隔离的档位不许被读高：手册与宪法都写着「不防越权」这一档（2026-10-01 §8.0.1）', () => {
  // 这一条防的是**过度承诺**：住户化买的是「防故障 ＋ 防误共享 ＋ 可分别起停」，
  // **不是**「住户之间互相看不见」。判据机械：两侧文档都必须把"没买到什么"写出来；
  // 谁把这段删了、只留"隔离"两个字，这里就红。
  const shape = fs.readFileSync(path.join(P.REPO, 'docs', 'constitution', 'PROJECT_SHAPE.md'), 'utf8')
  assert.ok(readme.includes('不防越权'), '手册没写清住户之间"不防越权"这一档')
  assert.ok(readme.includes('防故障'), '手册没写清买到的第一档是"防故障"')
  assert.ok(readme.includes('防误共享'), '手册没写清买到的第二档是"防误共享"')
  assert.ok(shape.includes('### 8.0.1 住户之间的隔离到哪一档'), '宪法里没有 §8.0.1 这一节')
  assert.ok(shape.includes('不防越权'), '宪法 §8.0.1 没写清"不防越权"这一档')
  assert.ok(shape.includes('看得见'), '宪法 §8.0.1 没写清可控性来自"看得见 ＋ 撤得掉"')
  // 四条触发条件（出现任一条 ⇒ 要么补墙、要么改宣称）：数得出四条编号。
  const section = shape.slice(shape.indexOf('### 8.0.1'))
  const body = section.slice(0, section.indexOf('### 8.1'))
  for (const marker of ['不再属于同一个主人', '不受控第三方', '安全隔离', '共享机器']) {
    assert.ok(body.includes(marker), `§8.0.1 的四条触发条件里少了这一条：${marker}`)
  }
})

docCase('手按支：手册与宪法都写着「手长在身体上」（支级声明、家级默认、读数说清来源）', () => {
  // 2026-10-01 第五刀：`mcp_*` / `terminal=` 从家级升成支级事实。这一条防的是**文档悄悄退回家级**
  // ——只要两侧还写着"支级声明 ＋ 家级默认 ＋ 三种来源 ＋ mcp=none/terminal=none 的说法"，
  // 用户就不会被"这支为什么没有终端"这种问题逼着去比对两个文件。
  const shape = fs.readFileSync(path.join(P.REPO, 'docs', 'constitution', 'PROJECT_SHAPE.md'), 'utf8')
  for (const marker of ['手长在身体上', 'mcp=none', 'terminal=none', '家级默认', '说清来源']) {
    assert.ok(readme.includes(marker), `手册没写清"手按支"的这一格：${marker}`)
    assert.ok(shape.includes(marker), `宪法 §8.0 没写清"手按支"的这一格：${marker}`)
  }
  // **半成品**（有目录、还没安家）：手册必须写清"`up --agent` 会把它安下来 / 裸 `up` 只跳过并说出来"
  // —— 这是真机上撞出来的那条路，用户得知道怎么走（2026-10-01 第五刀）。
  for (const marker of ['还没安家', 'up --agent']) {
    assert.ok(readme.includes(marker), `手册没写清"半成品住户"怎么安下来：${marker}`)
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 9, `用例数 ${cases} 少于声明的 9`)
})
