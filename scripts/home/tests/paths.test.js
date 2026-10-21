'use strict'
// 形态判据与路径：release（bin/ + client-dist/）vs source（target/release + client/dist）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const P = require('../paths')

let cases = 0
function pathCase(name, fn) {
  test(name, () => {
    fn()
    cases += 1
  })
}

function tempRepo() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'celira-repo-'))
}

function touch(file) {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  fs.writeFileSync(file, '', 'utf8')
}

/** 本平台上可执行件的后缀（Windows 是 `.exe`，别的平台没有）——用例按**真名**造件。 */
const EXE = process.platform === 'win32' ? '.exe' : ''
function touchExe(dir, base) {
  touch(path.join(dir, `${base}${EXE}`))
}

pathCase('一体包：bin/ 里有 Agent 的可执行件 ＋ core ⇒ 发布形态，且 hasCore', () => {
  const repo = tempRepo()
  touchExe(path.join(repo, 'bin'), 'agent_host')
  touchExe(path.join(repo, 'bin'), 'agent_host_status')
  touchExe(path.join(repo, 'bin'), 'agent_run')
  touchExe(path.join(repo, 'bin'), 'celira-im')
  touch(path.join(repo, 'client-dist', 'index.html'))
  const layout = P.detectLayout(repo)
  assert.equal(layout.kind, 'release')
  assert.equal(layout.hasCore, true)
  assert.equal(layout.imExe, path.join(repo, 'bin', `celira-im${EXE}`))
  assert.equal(layout.hostExe, path.join(repo, 'bin', `agent_host${EXE}`))
  assert.equal(layout.statusExe, path.join(repo, 'bin', `agent_host_status${EXE}`))
  assert.equal(layout.runExe, path.join(repo, 'bin', `agent_run${EXE}`))
  assert.equal(layout.clientDist, path.join(repo, 'client-dist'))
})

pathCase('**runtime 子包**：有 Agent 的可执行件、没有 core ⇒ 仍是发布形态，只是 hasCore=false', () => {
  // 这一格从前被误判成**源码形态**（判据锚在 core 在不在上）⇒ 反过来要用户 `cargo build`，
  // 还让他用 `\scripts\install.ps1`（Linux 上根本没有那个文件）。包里没有 core 是**按设计的
  // 形态**（runtime 子包只有 Agent 那一半），不是"坏了"。
  const repo = tempRepo()
  touchExe(path.join(repo, 'bin'), 'agent_host')
  touchExe(path.join(repo, 'bin'), 'agent_run')
  const layout = P.detectLayout(repo)
  assert.equal(layout.kind, 'release')
  assert.equal(layout.hasCore, false)
  assert.equal(layout.agentDir, path.join(repo, 'bin'))
  assert.equal(layout.hostExe, path.join(repo, 'bin', `agent_host${EXE}`))
  // 包里没有 core：`imExe` 指向 bin/ 里那个**不存在**的名字（读数要说"没有 core"，不是"源码树"）
  assert.equal(layout.imExe, path.join(repo, 'bin', `celira-im${EXE}`))
  assert.equal(layout.clientDist, path.join(repo, 'client-dist'))
})

pathCase('没有 bin/ ⇒ 源码形态（target/release + agent/target/release + client/dist）', () => {
  const repo = tempRepo()
  fs.mkdirSync(path.join(repo, 'client'), { recursive: true })
  const layout = P.detectLayout(repo)
  assert.equal(layout.kind, 'source')
  assert.equal(layout.hasCore, false)
  assert.equal(layout.imExe, path.join(repo, 'target', 'release', `celira-im${EXE}`))
  assert.equal(layout.agentDir, path.join(repo, 'agent', 'target', 'release'))
  assert.equal(layout.hostExe, path.join(repo, 'agent', 'target', 'release', `agent_host${EXE}`))
  assert.equal(layout.clientDist, path.join(repo, 'client', 'dist'))
  assert.equal(layout.clientDir, path.join(repo, 'client'))
})

pathCase('可执行件解析：.exe 优先；没有 .exe 就用同名无后缀（别的平台）', () => {
  const dir = tempRepo()
  touch(path.join(dir, 'plain-only'))
  assert.equal(P.exeIn(dir, 'plain-only'), path.join(dir, 'plain-only'))
  touch(path.join(dir, 'both.exe'))
  touch(path.join(dir, 'both'))
  assert.equal(P.exeIn(dir, 'both'), path.join(dir, 'both.exe'))
  // 两个都没有：返回**本平台**该有的名字——"缺可执行件"要说的是"这台机器上它该叫什么"
  // （从前一律返回 `.exe`，于是在 Linux 上报出 `celira-im.exe`）
  assert.equal(P.exeIn(dir, 'missing'), path.join(dir, `missing${EXE}`))
})

pathCase('isFile：目录不算文件（形态判据不能把目录当可执行件）', () => {
  const dir = tempRepo()
  fs.mkdirSync(path.join(dir, 'as-dir'))
  assert.ok(!P.isFile(path.join(dir, 'as-dir')))
  touch(path.join(dir, 'as-file'))
  assert.ok(P.isFile(path.join(dir, 'as-file')))
})

pathCase('家目录：CELIRA_HOME_DIR 覆盖，否则用默认值', () => {
  const before = process.env.CELIRA_HOME_DIR
  try {
    process.env.CELIRA_HOME_DIR = 'D:/celira-home-test'
    assert.equal(P.homeDir(), path.resolve('D:/celira-home-test'))
    delete process.env.CELIRA_HOME_DIR
    assert.equal(P.homeDir(), path.resolve(P.DEFAULT_HOME))
  } finally {
    if (before === undefined) delete process.env.CELIRA_HOME_DIR
    else process.env.CELIRA_HOME_DIR = before
  }
})

pathCase('家的默认位置按平台：Windows 是 D:/celira-home（一字未动）；别的平台是 ~/celira-home', () => {
  if (process.platform === 'win32') {
    assert.equal(P.DEFAULT_HOME, 'D:/celira-home')
  } else {
    assert.equal(P.DEFAULT_HOME, path.join(os.homedir(), 'celira-home'))
    // 为什么换掉 `D:/…`（2026-09-30）：POSIX 上它会被 resolve 成 `<cwd>/D:/celira-home`
    // —— 一个跟着当前目录跑的"家"。默认值必须**与 cwd 无关**。
    assert.ok(path.isAbsolute(P.DEFAULT_HOME))
    const before = process.env.CELIRA_HOME_DIR
    try {
      delete process.env.CELIRA_HOME_DIR
      assert.equal(P.homeDir(), path.join(os.homedir(), 'celira-home'))
    } finally {
      if (before !== undefined) process.env.CELIRA_HOME_DIR = before
    }
  }
})

pathCase('家目录：命令行声明的那一份（`--home`）压过环境变量 —— 它是常驻件的家事实', () => {
  const before = process.env.CELIRA_HOME_DIR
  try {
    process.env.CELIRA_HOME_DIR = 'D:/from-env'
    assert.equal(P.homeDir('D:/from-flag'), path.resolve('D:/from-flag'))
    // 空串 / 非字符串 = 没声明 ⇒ 退回环境变量（老行为一字不变）。
    assert.equal(P.homeDir(''), path.resolve('D:/from-env'))
    assert.equal(P.homeDir(undefined), path.resolve('D:/from-env'))
    assert.equal(P.homeDir(null), path.resolve('D:/from-env'))
  } finally {
    if (before === undefined) delete process.env.CELIRA_HOME_DIR
    else process.env.CELIRA_HOME_DIR = before
  }
})

// ── 2026-09-25：建新家时避开已被占用的默认口（两家同端口会让家事实退化）──

pathCase('新家端口：默认口都空着 ⇒ 一个都不动（现状一字不变）', () => {
  const { ports, moved } = P.chooseFreshPorts({
    defaults: { listen_port: 8080, client_port: 5173, status_port: 5190, config_port: 5191 },
    taken: () => false,
  })
  assert.deepEqual(ports, {
    listen_port: '8080',
    client_port: '5173',
    status_port: '5190',
    config_port: '5191',
  })
  assert.deepEqual(moved, [])
})

pathCase('新家端口：被占的那个往后挪一格，并如实报出来（其余不动）', () => {
  const { ports, moved } = P.chooseFreshPorts({
    defaults: { listen_port: 8080, client_port: 5173, status_port: 5190, config_port: 5191 },
    taken: (port) => port === 8080,
  })
  assert.equal(ports.listen_port, '8081')
  assert.equal(ports.client_port, '5173', '没被占的不许乱动')
  assert.deepEqual(moved, [{ key: 'listen_port', from: 8080, to: 8081 }])
})

pathCase('新家端口：连着几个都被占 ⇒ 跳到第一个空的；窗口用尽 ⇒ 如实报 exhausted（不假装挑到了）', () => {
  const cascade = P.chooseFreshPorts({
    defaults: { listen_port: 8080 },
    taken: (port) => port >= 8080 && port <= 8083,
  })
  assert.equal(cascade.ports.listen_port, '8084')
  assert.deepEqual(cascade.exhausted, [])

  const full = P.chooseFreshPorts({
    defaults: { listen_port: 8080, client_port: 5173 },
    taken: (port) => port >= 8080 && port <= 8083,
    span: 3,
  })
  assert.deepEqual(full.exhausted, [{ key: 'listen_port', base: 8080 }], '窗口里一个都没空着 ⇒ 报出来')
  assert.deepEqual(full.moved, [], '没挑到就不算"挪过"')
  assert.equal(full.ports.client_port, '5173', '别的口照旧（它本来就空着）')
})

// ── 2026-09-30：整组一起算 —— 两个门不许各自挑到同一格（登记的 bug）──

pathCase('新家端口：两个门的默认口都被占 ⇒ **互不相同**（不许各自挑到同一格）', () => {
  // 复现登记的 bug：只占 5190 与 5191 时，旧算法会让 status 与 config 都挑到 5192。
  const { ports, moved } = P.chooseFreshPorts({
    defaults: { listen_port: 8080, client_port: 5173, status_port: 5190, config_port: 5191 },
    taken: (port) => port === 5190 || port === 5191,
  })
  assert.equal(ports.status_port, '5192', '状态口往后找：跳过 5191（那是配置门的默认格）')
  assert.equal(ports.config_port, '5193', '配置门跳过已被本组占下的 5192')
  assert.notEqual(ports.status_port, ports.config_port, '两个门不许挑到同一格')
  assert.deepEqual(moved, [
    { key: 'status_port', from: 5190, to: 5192 },
    { key: 'config_port', from: 5191, to: 5193 },
  ])
})

pathCase('新家端口：四个口全被占（第二个家）⇒ 四个新口**两两不同**，且都在各自的默认口之后', () => {
  const takenSet = new Set([8080, 5173, 5190, 5191])
  const { ports, moved } = P.chooseFreshPorts({
    defaults: { listen_port: 8080, client_port: 5173, status_port: 5190, config_port: 5191 },
    taken: (port) => takenSet.has(port),
  })
  assert.deepEqual(ports, {
    listen_port: '8081',
    client_port: '5174',
    status_port: '5192',
    config_port: '5193',
  })
  assert.equal(new Set(Object.values(ports)).size, 4, '四个口两两不同')
  assert.deepEqual(moved.map((item) => item.key), ['listen_port', 'client_port', 'status_port', 'config_port'])
})

pathCase('新家端口：挪位不许抢走另一个口**空着的默认格**（它可能正是那一格的主人要留下的）', () => {
  // 只占 5190：旧算法会把 status 挪到 5191（配置门的默认格），配置门再留下 5191 ⇒ 撞格。
  const { ports, moved } = P.chooseFreshPorts({
    defaults: { status_port: 5190, config_port: 5191 },
    taken: (port) => port === 5190,
  })
  assert.equal(ports.status_port, '5192', '跳过别人的默认格')
  assert.equal(ports.config_port, '5191', '没被占的默认口照旧留给自己')
  assert.deepEqual(moved, [{ key: 'status_port', from: 5190, to: 5192 }])
})

pathCase('新家端口：全组不变量 —— 任意占用组合下，定下来的口都两两不同且避开被占的', () => {
  const defaults = { a: 5190, b: 5191, c: 5192, d: 5193 }
  for (const occupied of [[], [5190], [5190, 5191], [5191, 5192, 5193], [5190, 5191, 5192, 5193], [5192]]) {
    const takenSet = new Set(occupied)
    const { ports, exhausted } = P.chooseFreshPorts({ defaults, taken: (port) => takenSet.has(port) })
    const values = Object.values(ports).map(Number)
    assert.equal(new Set(values).size, values.length, `占用 ${occupied} 下不许撞格：${JSON.stringify(ports)}`)
    assert.deepEqual(exhausted, [])
    for (const value of values) {
      assert.ok(!takenSet.has(value), `占用 ${occupied} 下 ${value} 不该是被占的那个`)
    }
  }
})

pathCase('仓库根：本模块在 <repo>/scripts/home/ ⇒ 往上是仓库根', () => {
  assert.equal(P.REPO, path.resolve(__dirname, '..', '..', '..'))
  assert.ok(fs.existsSync(path.join(P.REPO, 'scripts', 'home', 'paths.js')))
})

pathCase('技能区路径：<家>/materials/skills（建家时连空目录一起建）', () => {
  const home = path.join('tmp-home')
  // 资料根是宿主 `--materials` 吃的那个；技能区在它下面，独占一层。
  assert.equal(P.materialsRoot(home), path.join(home, 'materials'))
  assert.equal(P.skillsRoot(home), path.join(home, 'materials', 'skills'))
  // 判据一句话：**没有技能区**（`None`，整块不出场）与**技能区空着**（`Some(0)`）是两件事，
  // 所以建家必须先把这个目录建出来——否则主体只看得见"没有那一行"。
  assert.equal(
    P.skillsRoot(home),
    path.join(P.materialsRoot(home), 'skills'),
    '技能区必须落在资料根下面（Agent 那边按 material_root.join("skills") 读）',
  )
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 5, `用例数 ${cases} 少于声明的 5`)
})
