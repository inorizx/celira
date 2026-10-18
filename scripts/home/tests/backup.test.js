'use strict'
// `node home.js backup`（2026-09-25）：**家的一次整份备份**。
//
// 这一族要钉住的是**判据**，不是"复制动作能跑"：
//  1. **前置条件**：还有东西在跑（账本里是我们的进程 / 世界的端口上有人在听）⇒ 拒绝
//     （运行中复制出来的库是"可能能用"，不是"能用"）；
//  2. **没干净停机**：`core.db` 旁的日志兄弟还在 ⇒ 拒绝（与 `tools/home_portability.py`
//     的 `db_unclean_stop` 同一条判据；本构建走 rollback journal ⇒ 真正常见的是 `-journal`）；
//  3. **不覆盖**：目的地已存在 ⇒ 拒绝；目的地在家里面 ⇒ 拒绝（自己套自己）；
//  4. **备的是什么**：默认**全备**（家的形状会长），只排除运行产物（`logs/`、`*.pid`）；
//  5. **双侧校验**：每个文件复制后源与落地各算一次 sha256，且清单最后写（半成品没有清单）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const crypto = require('node:crypto')
const fs = require('node:fs')
const net = require('node:net')
const os = require('node:os')
const path = require('node:path')

const backup = require('../backup')

let cases = 0
function bCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 临时家跑完就收拾的环境变量守卫。 */
async function withHome(home, fn) {
  const saved = process.env.CELIRA_HOME_DIR
  process.env.CELIRA_HOME_DIR = home
  try {
    return await fn()
  } finally {
    if (saved === undefined) delete process.env.CELIRA_HOME_DIR
    else process.env.CELIRA_HOME_DIR = saved
  }
}

/** 造一个临时家（真的落文件）。 */
function tempHome() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-backup-'))
  const home = path.join(root, 'celira-home')
  fs.mkdirSync(path.join(home, 'im', 'media', 'attachments'), { recursive: true })
  fs.mkdirSync(path.join(home, 'env'), { recursive: true })
  fs.mkdirSync(path.join(home, 'materials'), { recursive: true })
  fs.mkdirSync(path.join(home, 'logs'), { recursive: true })
  fs.writeFileSync(path.join(home, 'home.conf'), 'listen_addr=127.0.0.1:18080\nagent_id=a1\n')
  fs.writeFileSync(path.join(home, 'im', 'core.db'), 'SQLite format 3\0fake')
  fs.writeFileSync(path.join(home, 'im', 'media', 'attachments', 'x.bin'), Buffer.from([1, 2, 3]))
  fs.writeFileSync(path.join(home, 'env', 'identity.json'), '{"agent_id":"a1"}')
  fs.writeFileSync(path.join(home, 'materials', 'note.md'), 'hello')
  fs.writeFileSync(path.join(home, 'model_sources.json'), '[{"id":"m1","api_key":"SECRET"}]')
  fs.writeFileSync(path.join(home, 'logs', 'im.out'), 'runtime log')
  fs.writeFileSync(path.join(home, 'logs', 'im.pid'), '999999')
  return { root, home }
}

bCase('目的地默认在家**旁边**（绝不放进家里，否则自己套自己）', () => {
  const dest = backup.destinationFor('D:/celira-home', new Date('2026-09-25T10:30:45.123Z'))
  assert.equal(path.basename(dest), 'celira-home-backup-20260925-103045')
  assert.equal(path.dirname(dest), path.dirname(path.resolve('D:/celira-home')))
  assert.ok(!dest.startsWith(path.resolve('D:/celira-home') + path.sep), '不许落在家里')
  // 别的形态的家目录也一律解析成绝对路径再取其兄弟（平台上怎么写都成立）。
  const custom = backup.destinationFor('/srv/her', new Date('2026-01-02T03:04:05.000Z'))
  assert.equal(custom, path.join(path.dirname(path.resolve('/srv/her')), 'her-backup-20260102-030405'))
})

bCase('该跳过什么：运行产物（logs/ 与 *.pid）不进备份，其余**默认全备**', () => {
  assert.equal(backup.shouldSkip('logs/im.out'), true)
  assert.equal(backup.shouldSkip('logs/nested/deep.out'), true)
  assert.equal(backup.shouldSkip('service.pid'), true)
  assert.equal(backup.shouldSkip('logs/host.pid'), true)
  assert.equal(backup.shouldSkip(backup.MANIFEST_NAME), true, '自己那份清单不许进自己')
  assert.equal(backup.shouldSkip(''), true)
  // 家的东西一律要备 —— 包括**以后才出现的**新文件（默认全备才跟得上家的形状）
  assert.equal(backup.shouldSkip('home.conf'), false)
  assert.equal(backup.shouldSkip('im/core.db'), false)
  assert.equal(backup.shouldSkip('env/identity.json'), false)
  assert.equal(backup.shouldSkip('model_sources.json'), false, '模型清单也在家里 ⇒ 要备（它含 key，文档必须说清）')
  assert.equal(backup.shouldSkip('something-new-in-future/x'), false)
})

bCase('没干净停机的标志：三种后缀都认（本构建真正常见的是 `-journal`）', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-journal-'))
  const db = path.join(dir, 'core.db')
  fs.writeFileSync(db, 'x')
  assert.equal(backup.uncleanStopMarker(db), null, '没有兄弟文件 ⇒ 干净')
  for (const suffix of ['-journal', '-wal', '-shm']) {
    fs.writeFileSync(db + suffix, 'junk')
    assert.equal(backup.uncleanStopMarker(db), suffix, `${suffix} 必须被认出来`)
    fs.rmSync(db + suffix)
  }
  fs.rmSync(dir, { recursive: true, force: true })
})

bCase('listFiles：家被列全了，运行产物被排除（清单确定性 = 字典序）', () => {
  const { home } = tempHome()
  const files = backup.listFiles(home).map((f) => f.rel)
  assert.ok(files.includes('home.conf'))
  assert.ok(files.includes('im/core.db'))
  assert.ok(files.includes('im/media/attachments/x.bin'))
  assert.ok(files.includes('env/identity.json'))
  assert.ok(files.includes('materials/note.md'))
  assert.ok(files.includes('model_sources.json'))
  assert.ok(!files.some((f) => f.startsWith('logs/')), 'logs/ 是运行产物')
  assert.deepEqual(files, [...files].sort(), '按字典序（清单可复现）')
})

bCase('copyVerified：算的是**真内容**的 sha256（与独立算出来的那一个逐位相同）', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-copy-'))
  const from = path.join(dir, 'src.bin')
  const to = path.join(dir, 'dst.bin')
  const body = Buffer.from('hello celira')
  fs.writeFileSync(from, body)
  const ok = await backup.copyVerified(from, to)
  assert.equal(ok.ok, true)
  assert.equal(ok.size, body.length)
  // 不是"看着像"：拿独立算的摘要对（这条用例的意义就在这里）
  assert.equal(ok.sha256, crypto.createHash('sha256').update(body).digest('hex'))
  assert.equal(fs.readFileSync(to, 'utf8'), 'hello celira')
  // 负对照：源根本不存在 ⇒ **抛**（绝不静默返回"成了"）
  await assert.rejects(() => backup.copyVerified(path.join(dir, 'nope.bin'), path.join(dir, 'x.bin')))
  fs.rmSync(dir, { recursive: true, force: true })
})

bCase('★ 世界还在听那个端口 ⇒ 拒绝备份（前置条件是硬的，不做"大概其"的复制）', async () => {
  const { root, home } = tempHome()
  // 用**真的监听**来构造"她还在跑"：起一个 TCP 口，把它的端口写进 home.conf 的 listen_addr。
  // （为什么不往账本里塞一个 pid：家事实会认不出那不是本家的进程 —— 那是对的行为，
  //  所以"还在跑"要用**端口**这一维来构造，它是账本丢了也仍然成立的那一维。）
  const server = net.createServer()
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  fs.writeFileSync(path.join(home, 'home.conf'), `listen_addr=127.0.0.1:${port}\nagent_id=a1\n`)
  const dest = path.join(root, 'out')
  try {
    const code = await withHome(home, () => backup.backup({ to: dest }))
    assert.equal(code, 3, `端口上有世界在听 ⇒ 需要你动手（3），得到 ${code}`)
    assert.equal(fs.existsSync(dest), false, '被拒绝时不许留下半个目的地')
  } finally {
    await new Promise((resolve) => server.close(resolve))
    fs.rmSync(root, { recursive: true, force: true })
  }
})

bCase('★ 干净停机 ⇒ 备份成：全家的文件都在，清单最后写，且逐文件有 sha256', async () => {
  const { root, home } = tempHome()
  const dest = path.join(root, 'out')
  try {
    const code = await withHome(home, () => backup.backup({ to: dest }))
    assert.equal(code, 0, '干净停机 + 没有日志兄弟 ⇒ 应当备份成功')
    const manifestPath = path.join(dest, backup.MANIFEST_NAME)
    assert.ok(fs.existsSync(manifestPath), '清单要在')
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'))
    assert.equal(manifest.version, 1)
    assert.equal(manifest.source_home, home)
    assert.ok(manifest.totals.files > 0)
    assert.ok(manifest.totals.bytes > 0)
    for (const entry of manifest.files) {
      assert.match(entry.sha256, /^[0-9a-f]{64}$/, `${entry.path} 要有 sha256`)
      assert.ok(fs.existsSync(path.join(dest, entry.path)), `${entry.path} 要真的在备份里`)
    }
    const files = manifest.files.map((f) => f.path)
    assert.ok(files.includes('home.conf'))
    assert.ok(files.includes('im/core.db'))
    assert.ok(files.includes('env/identity.json'))
    assert.ok(files.includes('model_sources.json'), '模型清单要备 —— 它是家里的东西（含 key，文档说清）')
    assert.ok(!files.some((f) => f.startsWith('logs/')))
    // 内容真的一字不差（不是只看文件在不在）
    assert.equal(
      fs.readFileSync(path.join(dest, 'env', 'identity.json'), 'utf8'),
      fs.readFileSync(path.join(home, 'env', 'identity.json'), 'utf8'),
    )
    // 清单**自己**不在清单里
    assert.ok(!files.includes(backup.MANIFEST_NAME))
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

bCase('★ 没干净停机（core.db-journal 还在）⇒ 拒绝；目的地已存在 / 在家里 ⇒ 也拒绝', async () => {
  const { root, home } = tempHome()
  fs.writeFileSync(path.join(home, 'im', 'core.db-journal'), 'junk')
  try {
    const dest = path.join(root, 'out')
    assert.equal(await withHome(home, () => backup.backup({ to: dest })), 3, '没干净停机 ⇒ 需要你动手（3）')
    assert.equal(fs.existsSync(dest), false)

    // 拿掉日志兄弟 ⇒ 这次该成；再把同一个目的地跑第二遍 ⇒ **绝不覆盖**
    fs.rmSync(path.join(home, 'im', 'core.db-journal'))
    assert.equal(await withHome(home, () => backup.backup({ to: dest })), 0)
    assert.equal(await withHome(home, () => backup.backup({ to: dest })), 2, '目的地已存在 ⇒ 环境不可用（2）')

    // 目的地在家里面 ⇒ 用法错（自己套自己）
    assert.equal(await withHome(home, () => backup.backup({ to: path.join(home, 'inner') })), 1)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

bCase('★ 参数：`--to` 缺值/空值 ⇒ 用法错，**绝不回落成默认目的地**', async () => {
  // 真机踩到过这个：`node home.js backup --to`（脚本里变量为空）被读成"备份到家旁边那个
  // 默认目录" ⇒ 一次没人要的整份复制，半成品落在磁盘根上。缺值必须报用法错。
  assert.deepEqual(backup.parseArgs([]), { ok: true })
  assert.deepEqual(backup.parseArgs(['--to', 'D:/bk']), { ok: true, to: 'D:/bk' })
  assert.equal(backup.parseArgs(['--to']).ok, false, '缺值 ⇒ 用法错')
  assert.equal(backup.parseArgs(['--to', '']).ok, false, '空串 ⇒ 用法错')
  assert.equal(backup.parseArgs(['--to', '   ']).ok, false, '空白 ⇒ 用法错')
  assert.equal(backup.parseArgs(['--to', '--force']).ok, false, '下一个旗标不算值 ⇒ 用法错')
  assert.equal(backup.parseArgs(['--force']).ok, false, '认不得的参数 ⇒ 用法错')
  // 兜底那一层（就算调用方绕开 `parseArgs`）：空 `to` 一律用法错，绝不当成"没给"。
  const { home } = tempHome()
  try {
    assert.equal(await withHome(home, () => backup.backup({ to: '' })), 1)
    assert.equal(await withHome(home, () => backup.backup({ to: '   ' })), 1)
  } finally {
    fs.rmSync(home, { recursive: true, force: true })
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '一条用例都没跑（空跑不算过）')
  assert.equal(cases, 9, `用例数 ${cases} 与声明的 9 不符`)
})
