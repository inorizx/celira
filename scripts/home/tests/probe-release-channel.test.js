/**
 * 发布渠道探针的用例（`scripts/probe-release-channel.mjs`）。
 *
 * ⚠ **为什么这份用例住在 `scripts/home/tests/`**：仓里只有这一处 `node --test` 的家
 * （`node --test "scripts/home/tests/*.test.js"`，每个人都会跑的那条）。探针本体在 `scripts/`
 * 根下（与发布那一侧的 `make_release.ps1` 作伴），这里只放它的判据 —— 别为一份用例新造一个
 * 没人跑的测试根。
 *
 * 判据分两层，与探针自己的口径一致：
 * ① **纯函数**（读版本 / 比版本 / 读渠道答的那三格）——直接量，不碰网络；
 * ② **端到端跑一次 CLI**——对着离线夹具跑（**这条探针的判据必须能在没有网络的机器上被证明**），
 *    真网那条只在演示时手动跑，不进套件。
 */
const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { execFileSync } = require('node:child_process')

const PROBE = path.resolve(__dirname, '..', '..', 'probe-release-channel.mjs')
const probe = require(PROBE)

function tempTree(tag) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-channel-${tag}-`))
}

function runProbe(args) {
  try {
    const stdout = execFileSync(process.execPath, [PROBE, ...args], { encoding: 'utf8', stdio: 'pipe' })
    return { code: 0, out: stdout }
  } catch (error) {
    return { code: error.status ?? 1, out: `${error.stdout ?? ''}${error.stderr ?? ''}` }
  }
}

/** 一份 Gitee 列表端点的夹具（只留我们读数用到的字段）。 */
function releaseFixture({ tag, created, assets }) {
  return [
    {
      id: 1,
      tag_name: tag,
      name: `Celira ${tag}`,
      created_at: created,
      assets,
    },
  ]
}

test('读在跑的版本：从包的 VERSION 里取 version 那一行', () => {
  const dir = tempTree('version')
  fs.writeFileSync(path.join(dir, 'VERSION'), 'version = 0.1.0\ngit rev = abc1234\nbuilt = x\n', 'utf8')
  assert.deepEqual(probe.versionFromPackage(dir), { ok: true, version: '0.1.0' })
  const empty = tempTree('version-missing')
  assert.equal(probe.versionFromPackage(empty).ok, false)
  fs.rmSync(dir, { recursive: true, force: true })
  fs.rmSync(empty, { recursive: true, force: true })
})

test('比版本：够用就够用，不发明 semver 库', () => {
  assert.equal(probe.compareVersions('0.1.0', '0.1.1'), -1)
  assert.equal(probe.compareVersions('0.1.1', '0.1.0'), 1)
  assert.equal(probe.compareVersions('0.1.0', '0.1.0'), 0)
  assert.equal(probe.compareVersions('0.2.0', '0.10.0'), -1, '数字段按数比，不按字符串')
  assert.equal(probe.compareVersions('0.1.0', '0.1.0-dirty'), -1, '脏树那一版算在干净 tag 之后')
})

test('渠道答的三格：取最新那一版，并把产物地址挑出来', () => {
  const answer = probe.readChannel(
    releaseFixture({
      tag: 'v0.1.1',
      created: '2026-10-12T00:00:00+08:00',
      assets: [{ name: 'celira-0.1.1-win-x64.zip', browser_download_url: 'https://x/y.zip', size: 123 }],
    }),
  )
  assert.equal(answer.ok, true, JSON.stringify(answer))
  assert.equal(answer.entry.version, '0.1.1', 'v 前缀不算版本的一部分')
  assert.equal(answer.entry.asset.url, 'https://x/y.zip')
  assert.equal(answer.entry.asset.size, 123)
})

test('渠道答的三格：最新按时间排（不是按数组顺序）', () => {
  const answer = probe.readChannel([
    { tag_name: 'v0.1.2', created_at: '2026-10-01T00:00:00Z', assets: [{ name: 'a.zip', browser_download_url: 'u' }] },
    { tag_name: 'v0.1.1', created_at: '2026-10-12T00:00:00Z', assets: [{ name: 'b.zip', browser_download_url: 'u' }] },
  ])
  assert.equal(answer.entry.version, '0.1.1', '时间新的那一版才算最新')
})

test('渠道答不上时分开说：没有发行版＝不适用；缺产物地址＝形状读不懂', () => {
  const empty = probe.readChannel([])
  assert.equal(empty.ok, false)
  assert.equal(empty.notApplicable, true, '一个发行版都没有 ⇒ 不适用（不是失败），但它是结论')
  const noAssetUrl = probe.readChannel(releaseFixture({ tag: 'v0.1.1', created: 'x', assets: [{ name: 'a.zip' }] }))
  assert.equal(noAssetUrl.ok, false)
  assert.equal(noAssetUrl.notApplicable, false, '答了但缺字段 ⇒ 形状读不懂')
  assert.match(noAssetUrl.why, /下载地址/)
  const notAList = probe.readChannel({ nope: true })
  assert.equal(notAList.ok, false)
  assert.equal(notAList.notApplicable, false)
})

test('端到端：对着离线夹具跑，说有新版；把"你在跑哪一版"也算进去', () => {
  const dir = tempTree('fixture')
  const fixture = path.join(dir, 'releases.json')
  fs.writeFileSync(
    fixture,
    JSON.stringify(
      releaseFixture({
        tag: 'v0.1.1',
        created: '2026-10-12T00:00:00+08:00',
        assets: [{ name: 'celira-0.1.1-win-x64.zip', browser_download_url: 'https://gitee.com/x/y.zip', size: 29200000 }],
      }),
    ),
    'utf8',
  )
  const done = runProbe(['--current', '0.1.0', '--channel', fixture, '--repo', 'owner/celira'])
  assert.equal(done.code, 0, done.out)
  assert.match(done.out, /\[app\]\s+你在跑 0\.1\.0/)
  assert.match(done.out, /最新发行版：0\.1\.1/)
  assert.match(done.out, /有新版：0\.1\.0 → 0\.1\.1/)
  assert.match(done.out, /换法：下载 → 解压到新文件夹/)
  assert.match(done.out, /没量的那一处/, '不会替你判的那一处要说出来')
  fs.rmSync(dir, { recursive: true, force: true })
})

test('端到端：夹具里一个发行版都没有 ⇒ 退出 2（不适用，不是失败）', () => {
  const dir = tempTree('empty')
  const fixture = path.join(dir, 'releases.json')
  fs.writeFileSync(fixture, '[]', 'utf8')
  const done = runProbe(['--current', '0.1.0', '--channel', fixture])
  assert.equal(done.code, 2, done.out)
  assert.match(done.out, /查不到（不适用，不是失败）：这个仓库还没有任何发行版/)
  fs.rmSync(dir, { recursive: true, force: true })
})

test('端到端：不给"你在跑哪一版" ⇒ 用法错（退出 1）', () => {
  const done = runProbe(['--channel', 'gitee'])
  assert.equal(done.code, 1, done.out)
  assert.match(done.out, /--current <版本>.*或.*--package <包目录>/)
})
