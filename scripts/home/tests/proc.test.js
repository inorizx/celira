'use strict'
// 进程/端口判据（纯函数为主）：切词、`--mcp` 名字、netstat 解析、pid 归属。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')

const proc = require('../proc')

let cases = 0
function procCase(name, fn) {
  test(name, () => {
    fn()
    cases += 1
  })
}

/** 真机（要 I/O）的用例：**必须 await** —— 漏掉它断言会被静默跳过、还假装通过。 */
function procAsyncCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/**
 * 家事实（认领判据的第二维）——测试里的"**本家**"：`D:\celira` 那一家。
 * `listening` 是端口快照（`Map<pid, Set<端口>>`）：谁在听哪个口。
 */
function homeOf(overrides = {}) {
  return {
    homeDir: 'D:\\celira-home',
    envDir: 'D:\\celira\\env',
    clientPort: '5173',
    imPort: '8080',
    statusPort: '5190',
    configPort: '5191',
    listening: new Map(),
    ...overrides,
  }
}

/** 另一家的家事实（同一份发布包！只有家事实不同）——"两家并存"用例的另一半。 */
function otherHome(listening = new Map()) {
  return homeOf({
    homeDir: 'F:\\celira-test\\home',
    envDir: 'F:\\celira-test\\home\\env',
    clientPort: '5273',
    imPort: '8081',
    statusPort: '5290',
    configPort: '5291',
    listening,
  })
}

procCase('切词：引号里的空格不算分隔（`--mcp "名字=命令 带空格"` 是一个取值）', () => {
  assert.deepEqual(proc.parseTokens('agent_host.exe --root C:\\a --mcp "browser=stdio:npx a b" --x'), [
    'agent_host.exe',
    '--root',
    'C:\\a',
    '--mcp',
    'browser=stdio:npx a b',
    '--x',
  ])
})

procCase('切词：连续空格、首尾空格、空串都不产生空取值', () => {
  assert.deepEqual(proc.parseTokens('  a   b  '), ['a', 'b'])
  assert.deepEqual(proc.parseTokens(''), [])
  assert.deepEqual(proc.parseTokens('   '), [])
})

procCase('--mcp 名字：按出现顺序、去重、`--mcp` 在末尾不炸、别的参数里的等号不算', () => {
  const line =
    '"C:\\x\\agent_host.exe" --root C:\\h\\env --mcp "browser=stdio:npx a" --im-endpoint http://127.0.0.1:8080 ' +
    '--mcp "notes=stdio:cmd,read-only" --mcp "browser=stdio:again" --mcp'
  assert.deepEqual(proc.mcpNamesFromCommandLine(line), ['browser', 'notes'])
  assert.deepEqual(proc.mcpNamesFromCommandLine('agent_host.exe --root a'), [])
})

procCase('netstat：只认 LISTENING，端口必须**精确**（8080 不匹配 18080）', () => {
  const output = [
    '  TCP    127.0.0.1:8080         0.0.0.0:0              LISTENING       28408',
    '  TCP    127.0.0.1:18080        0.0.0.0:0              LISTENING       11111',
    '  TCP    127.0.0.1:8080         127.0.0.1:51209        ESTABLISHED     28408',
    '  UDP    0.0.0.0:8080           *:*                                    22222',
    '  TCP    [::1]:5173             [::]:0                 LISTENING       28700',
  ].join('\r\n')
  assert.deepEqual(proc.parseNetstat(output, '8080'), [28408])
  assert.deepEqual(proc.parseNetstat(output, '5173'), [28700])
  assert.deepEqual(proc.parseNetstat(output, '18080'), [11111])
  assert.deepEqual(proc.parseNetstat(output, '9999'), [])
})

procCase('netstat：同一端口多个 pid 去重并排序', () => {
  const output = [
    '  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       900',
    '  TCP    [::]:80                [::]:0                 LISTENING       4',
    '  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       900',
  ].join('\n')
  assert.deepEqual(proc.parseNetstat(output, '80'), [4, 900])
})

procCase('路径比较：Windows 上大小写与斜杠方向都无关；POSIX 上大小写敏感、`/` 分隔', () => {
  if (proc.IS_WINDOWS) {
    assert.ok(proc.samePath('D:\\celira\\bin\\celira-im.exe', 'd:/celira/bin/celira-im.exe'))
    assert.ok(!proc.samePath('D:\\celira\\bin\\celira-im.exe', 'D:\\other\\bin\\celira-im.exe'))
    assert.ok(!proc.samePath(null, 'D:\\celira\\bin\\celira-im.exe'))
  } else {
    // POSIX：大小写是**两个东西**（`/a/B` 与 `/a/b` 不是同一个路径）；反斜杠不是分隔符。
    assert.ok(proc.samePath('/srv/celira/bin/celira-im', '/srv/celira/bin/celira-im'))
    assert.ok(!proc.samePath('/srv/celira/bin/celira-im', '/srv/Celira/bin/celira-im'))
    assert.ok(!proc.samePath('/srv/celira/bin/celira-im', '\\srv\\celira\\bin\\celira-im'), '反斜杠不许被互译成 `/`')
    assert.ok(proc.samePath('/srv/celira/bin/', '/srv/celira/bin'), '末尾斜杠归一（同一个目录的两种写法）')
    assert.ok(!proc.samePath(null, '/srv/celira/bin/celira-im'))
  }
})

procCase('★ 路径比较：**同一个目录的另一种写法**也算同一个（短名 / `..`）—— 2026-10-11 立的判据', () => {
  // 真撞上的代价（不是理论）：`spawn` 出来的命令行里路径是 **8.3 短名**
  // （实测 `--home D:\CE76E2~1\celira-temp-world-…`，而本家手里是长名）⇒ 逐字比认不出
  // "这是我的门" ⇒ `home.js down` **拒绝杀自己起的门** ⇒ 门留在盘上占着 5190/5191 ⇒
  // 后面每一次产品形态门禁量的都是它（看着像代码坏了）。
  //
  // ⚠ 这条用例用 `..` 造"另一种写法"（可移植，且不依赖卷上有没有开 8.3 —— 两者走的是
  //   同一条路：交给文件系统解析一次）。真机上那个短名的读数记在提交正文里。
  const fs = require('node:fs')
  const os = require('node:os')
  const path = require('node:path')
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-samepath-'))
  // ⚠ **不许用 `path.join` 造带 `..` 的写法**：它会把 `..` 归一掉 ⇒ 又变回同一个字符串，
  //   用例就成了空过（这一条自己先空过了一次，如实记着）。
  const withDots = (...parts) => [dir, ...parts].join(path.sep)
  try {
    fs.mkdirSync(path.join(dir, 'sub'))
    assert.ok(
      proc.samePath(withDots('sub', '..'), dir),
      '带 `..` 的写法与直写必须是同一个目录（否则 down 认不出自己起的门）',
    )
    assert.ok(!proc.samePath(path.join(dir, '..'), dir), '真的不同目录不许被认成同一个')
    // ⚠ **解析不出来就不认领**：宁可杀不掉，也不误杀（这条判据错一次的代价是杀掉别人的东西）。
    //   实测（2026-10-11）：Windows 上 `realpath` 对 `…\nope\..` 会**先按字面归一**再解析 ⇒
    //   中间那段不存在也照样成功 —— 所以"解析不出来"这一档在 Windows 上**造不出**这个例子，
    //   这里只用一条解不出来的路径钉住"不认领"这半边。
    assert.ok(!proc.samePath(path.join(dir, 'gone'), dir), '解析不出来的路径 ⇒ 不许认领')
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

procCase('命令行包含判据：Windows 上大小写无关；POSIX 上按字面', () => {
  if (proc.IS_WINDOWS) {
    assert.ok(proc.cmdlineIncludes('node scripts/Serve_Client.mjs --port 5173', 'serve_client.mjs'))
  } else {
    assert.ok(proc.cmdlineIncludes('node scripts/serve_client.mjs --port 5173', 'serve_client.mjs'))
    assert.ok(
      !proc.cmdlineIncludes('node scripts/Serve_Client.mjs --port 5173', 'serve_client.mjs'),
      'POSIX 上大小写敏感（大小写不同就是不同的片段）',
    )
  }
  assert.ok(!proc.cmdlineIncludes('node other.mjs', 'serve_client.mjs'))
  assert.ok(!proc.cmdlineIncludes(null, 'serve_client.mjs'))
})

procCase('命令行旗标取值：`--旗标 值` 成对比（引号路径、大小写/斜杠无关、更长路径不算中）', () => {
  assert.ok(proc.cmdlineFlagMatches('a.exe --root D:\\celira\\env --x', '--root', 'D:\\celira\\env'))
  if (proc.IS_WINDOWS) {
    assert.ok(proc.cmdlineFlagMatches('a.exe --root "F:\\my home\\env"', '--root', 'F:/my home/env'))
  } else {
    assert.ok(proc.cmdlineFlagMatches('a.exe --root "/home/my home/env"', '--root', '/home/my home/env'))
  }
  assert.ok(!proc.cmdlineFlagMatches('a.exe --root D:\\celira\\env2', '--root', 'D:\\celira\\env'), '更长的路径不许被前缀算中')
  assert.ok(!proc.cmdlineFlagMatches('a.exe --root D:\\celira\\env', '--root', 'D:\\celira\\en'))
  assert.ok(!proc.cmdlineFlagMatches('a.exe --root', '--root', 'D:\\celira\\env'), '旗标在末尾、没有取值 ⇒ 不算')
  assert.ok(!proc.cmdlineFlagMatches(null, '--root', 'x'))
  assert.ok(!proc.cmdlineFlagMatches('a.exe --other D:\\celira\\env', '--root', 'D:\\celira\\env'))
  // 片段出现在别的取值里（`--mcp` 的命令）不算：判据只认成对的旗标。
  assert.ok(!proc.cmdlineFlagMatches('a.exe --mcp "x=npx --root D:\\celira\\env"', '--root', 'D:\\celira\\env'))
  // POSIX 的**精确 argv**（`/proc/<pid>/cmdline` 给的那份）：含空格的取值不靠引号也切得对。
  assert.ok(
    proc.cmdlineFlagMatches(['a.exe', '--root', '/home/my home/env', '--x'], '--root', '/home/my home/env'),
  )
  assert.ok(
    !proc.cmdlineFlagMatches(['a.exe', '--root', '/home/my home/env2'], '--root', '/home/my home/env'),
    'argv 形态同样只认成对相等（前缀不算中）',
  )
})

procCase('端口快照：只收 LISTENING，一个 pid 多个口都要，IPv6 形态也认', () => {
  const output = [
    '  TCP    127.0.0.1:8080         0.0.0.0:0              LISTENING       28408',
    '  TCP    [::]:5190              [::]:0                 LISTENING       900',
    '  TCP    127.0.0.1:8080         127.0.0.1:51209        ESTABLISHED     28408',
    '  UDP    0.0.0.0:8080           *:*                                    22222',
    '  TCP    0.0.0.0:5290            0.0.0.0:0              LISTENING       900',
    '  TCP    127.0.0.1:8081         0.0.0.0:0              LISTENING       abc',
  ].join('\r\n')
  const map = proc.listeningPortsByPid(output)
  assert.deepEqual([...map.get(28408)].sort(), [8080])
  assert.deepEqual([...map.get(900)].sort(), [5190, 5290], '同一个 pid 的两个口都要在')
  assert.ok(!map.has(22222), 'UDP 与 ESTABLISHED 都不算"在听"')
  assert.ok(!map.has(NaN))
})

procCase('pid 归属：im 按可执行路径（＋端口），agent 按可执行路径＋`--root`，认不出就不杀', () => {
  const layout = proc.IS_WINDOWS
    ? {
        imExe: 'D:\\celira\\target\\release\\celira-im.exe',
        hostExe: 'D:\\celira\\agent\\target\\release\\agent_host.exe',
        clientDir: 'D:\\celira\\client',
        repo: 'D:\\celira',
      }
    : {
        imExe: '/srv/celira/target/release/celira-im',
        hostExe: '/srv/celira/agent/target/release/agent_host',
        clientDir: '/srv/celira/client',
        repo: '/srv/celira',
      }
  const home = homeOf(
    proc.IS_WINDOWS ? {} : { homeDir: '/srv/celira-home', envDir: '/srv/celira-home/env' },
  )
  assert.ok(proc.ownsPid('im', { pid: 11, exePath: layout.imExe }, layout, home))
  if (proc.IS_WINDOWS) {
    // Windows：同一个文件的多种写法（大小写/斜杠方向）都算自己人。
    assert.ok(proc.ownsPid('im', { pid: 11, exePath: 'd:/celira/target/release/celira-im.exe' }, layout, home))
  }
  assert.ok(!proc.ownsPid('im', { pid: 11, exePath: layout.hostExe }, layout, home))
  // 宿主：路径对 ＋ `--root` 是本家的环境根 —— 两条都要。
  const hostLine = `agent_host --root ${home.envDir} --agent x --im-endpoint http://127.0.0.1:8080`
  assert.ok(proc.ownsPid('agent', { exePath: layout.hostExe, cmdline: hostLine }, layout, home))
  assert.ok(!proc.ownsPid('agent', { exePath: layout.hostExe, cmdline: 'agent_host --agent x' }, layout, home), '没有 --root 不算自己人')
  assert.ok(!proc.ownsPid('agent', { exePath: layout.hostExe, cmdline: `agent_host --root ${otherHome().envDir}` }, layout, home), '别家的环境根不算')
  assert.ok(!proc.ownsPid('agent', { exePath: layout.imExe, cmdline: hostLine }, layout, home))
  assert.ok(!proc.ownsPid('agent', null, layout, home))
  // 宿主那支的**精确 argv** 形态（POSIX 的 /proc 给的就是它）：同一套判据照旧成立。
  assert.ok(
    proc.ownsPid(
      'agent',
      { exePath: layout.hostExe, cmdline: hostLine, argv: ['agent_host', '--root', home.envDir, '--agent', 'x'] },
      layout,
      home,
    ),
  )
})

procCase('pid 归属：client 必须既像壳（node/cmd）、提到我们的东西，又带**本家的 `--port`**', () => {
  const win = proc.IS_WINDOWS
  const layout = win
    ? {
        imExe: 'D:\\celira\\target\\release\\celira-im.exe',
        hostExe: 'D:\\celira\\agent\\target\\release\\agent_host.exe',
        clientDir: 'D:\\celira\\client',
        repo: 'D:\\celira',
      }
    : {
        imExe: '/srv/celira/target/release/celira-im',
        hostExe: '/srv/celira/agent/target/release/agent_host',
        clientDir: '/srv/celira/client',
        repo: '/srv/celira',
      }
  const home = homeOf(win ? {} : { homeDir: '/srv/celira-home' })
  const shellName = win ? 'node.exe' : 'node'
  const repo = win ? 'D:\\celira' : '/srv/celira'
  assert.ok(
    proc.ownsPid(
      'client',
      { name: shellName, cmdline: `node ${repo}${win ? '\\' : '/'}scripts${win ? '\\' : '/'}serve_client.mjs --port 5173` },
      layout,
      home,
    ),
  )
  assert.ok(
    proc.ownsPid(
      'client',
      {
        name: shellName,
        cmdline: win
          ? '"F:\\node\\node.exe" D:\\celira\\client\\node_modules\\vite\\bin\\vite.js preview --port 5173 --strictPort'
          : '/usr/bin/node /srv/celira/client/node_modules/vite/bin/vite.js preview --port 5173 --strictPort',
      },
      layout,
      home,
    ),
  )
  // 系统里另一个 node 进程：不是我们的，不许杀。
  assert.ok(!proc.ownsPid('client', { name: shellName, cmdline: `node ${win ? 'C:\\other\\thing.js' : '/other/thing.js'}` }, layout, home))
  assert.ok(!proc.ownsPid('client', { name: 'chrome.exe', cmdline: `chrome ${repo}` }, layout, home))
  // ★ TASK_59 收紧：**只提 repo 不再算数**（那一条正是把状态口认成 client 的入口）。
  assert.ok(
    !proc.ownsPid('client', { name: shellName, cmdline: `node ${repo}${win ? '\\' : '/'}home.js status --serve` }, layout, home),
    '状态口命令行的两个标记都不该落进 client 支',
  )
  assert.ok(
    !proc.ownsPid('client', { name: shellName, cmdline: `node ${repo}${win ? '\\' : '/'}scripts${win ? '\\' : '/'}something.mjs` }, layout, home),
    '光提 repo 的 node 进程不再被认成 client',
  )
  // 2026-09-24：**别家的 client**（同一个 serve_client.mjs，但 `--port` 是别家那个）不算自己人。
  assert.ok(
    !proc.ownsPid(
      'client',
      {
        name: shellName,
        cmdline: win
          ? 'node D:\\celira\\scripts\\serve_client.mjs --dist D:\\celira\\client-dist --port 5273'
          : 'node /srv/celira/scripts/serve_client.mjs --dist /srv/celira/client-dist --port 5273',
      },
      layout,
      home,
    ),
    '别家的 client 端口不算中',
  )
})

// ── TASK_59：状态口是第四个件，指纹必须能与 client 分开 ──────────────────────
procCase('pid 归属：状态口（node home.js status --serve）只有 status 支认', () => {
  const layout = {
    imExe: 'D:\\celira\\target\\release\\celira-im.exe',
    hostExe: 'D:\\celira\\agent\\target\\release\\agent_host.exe',
    clientDir: 'D:\\celira\\client',
    repo: 'D:\\celira',
  }
  const statusInfo = { name: 'node.exe', cmdline: 'node D:\\celira\\home.js status --serve' }
  const home = homeOf()
  assert.ok(proc.ownsPid('status', statusInfo, layout, home))
  assert.ok(!proc.ownsPid('client', statusInfo, layout, home), '两支不许互相认领')
  // 反方向：client 的命令行也不该被 status 支认。
  const clientInfo = { name: 'node.exe', cmdline: 'node D:\\celira\\scripts\\serve_client.mjs --port 5173' }
  assert.ok(proc.ownsPid('client', clientInfo, layout, home))
  assert.ok(!proc.ownsPid('status', clientInfo, layout, home))
  // `home.js` 的**一次性命令**不是常驻件：少了 `--serve` 就不认（否则 up/down 自己会被认领）。
  assert.ok(!proc.ownsPid('status', { name: 'node.exe', cmdline: 'node D:\\celira\\home.js up' }, layout, home))
  assert.ok(!proc.ownsPid('status', { name: 'node.exe', cmdline: 'node D:\\celira\\home.js status' }, layout, home))
})

// ── TASK_77：模型配置门是第五个件，指纹必须与**同为 `home.js … --serve` 的状态口**分开 ──
procCase('pid 归属：模型配置门（node home.js config --serve）只有 config 支认', () => {
  const layout = {
    imExe: 'D:\\celira\\target\\release\\celira-im.exe',
    hostExe: 'D:\\celira\\agent\\target\\release\\agent_host.exe',
    clientDir: 'D:\\celira\\client',
    repo: 'D:\\celira',
  }
  const configInfo = { name: 'node.exe', cmdline: 'node D:\\celira\\home.js config --serve' }
  const statusInfo = { name: 'node.exe', cmdline: 'node D:\\celira\\home.js status --serve' }
  const home = homeOf()
  assert.ok(proc.ownsPid('config', configInfo, layout, home))
  // **两支不许互相认领**：它们都是 `home.js … --serve`，只差那个子命令词。
  assert.ok(!proc.ownsPid('status', configInfo, layout, home), '状态口不许认到模型门上')
  assert.ok(!proc.ownsPid('config', statusInfo, layout, home), '模型门不许认到状态口上')
  assert.ok(!proc.ownsPid('client', configInfo, layout, home))
  // 一次性命令（少了 `--serve`）不是常驻件，不认。
  assert.ok(!proc.ownsPid('config', { name: 'node.exe', cmdline: 'node D:\\celira\\home.js config' }, layout, home))
  assert.ok(!proc.ownsPid('config', { name: 'node.exe', cmdline: 'node D:\\celira\\home.js up' }, layout, home))
})

// ── 2026-09-24：同一份发布包服务多个家 ⇒ 判据必须有**家事实**这一维 ─────────────
procCase('两家并存·宿主：同一个 agent_host.exe、同一个路径，只认 `--root` 是本家的那支', () => {
  const layout = { hostExe: 'F:\\celira-test\\bin\\agent_host.exe', imExe: 'F:\\celira-test\\bin\\celira-im.exe', clientDir: 'F:\\celira-test\\client', repo: 'F:\\celira-test' }
  const real = homeOf({ envDir: 'D:\\celira-home\\env' })       // 真家（跑着）
  const temp = otherHome()                                     // 临时家（本家）
  const realHost = { pid: 33828, exePath: layout.hostExe, cmdline: `${layout.hostExe} --root D:\\celira-home\\env --agent a --im-endpoint http://127.0.0.1:8080` }
  const tempHost = { pid: 40000, exePath: layout.hostExe, cmdline: `${layout.hostExe} --root F:\\celira-test\\home\\env --agent b --im-endpoint http://127.0.0.1:8081` }
  // 负对照（本刀的症状本体）：临时家**不许**认领真家的宿主 —— 认了就是"up 不起本家、down 跨家误杀"。
  assert.ok(!proc.ownsPid('agent', realHost, layout, temp), '别家的宿主不算自己人')
  assert.ok(!proc.ownsPid('agent', tempHost, layout, real), '反方向同理')
  // 正对照：各认各的。
  assert.ok(proc.ownsPid('agent', realHost, layout, real))
  assert.ok(proc.ownsPid('agent', tempHost, layout, temp))
})

procCase('两家并存·端口维（im/status/config）：在听**别的**端口 ⇒ 否认；谁都没听 ⇒ 保留老判据', () => {
  const layout = {
    imExe: 'F:\\celira-test\\bin\\celira-im.exe',
    hostExe: 'F:\\celira-test\\bin\\agent_host.exe',
    clientDir: 'F:\\celira-test\\client',
    repo: 'F:\\celira-test',
  }
  const temp = otherHome(new Map([
    [21072, new Set([8080])], // 真家的 im（在 8080 上听）
    [16360, new Set([5190])], // 真家的状态口
    [29692, new Set([5191])], // 真家的模型门
  ]))
  const imOfReal = { pid: 21072, exePath: layout.imExe, cmdline: layout.imExe }
  const statusOfReal = { pid: 16360, name: 'node.exe', cmdline: 'node F:\\celira-test\\home.js status --serve' }
  const configOfReal = { pid: 29692, name: 'node.exe', cmdline: 'node F:\\celira-test\\home.js config --serve' }
  assert.ok(!proc.ownsPid('im', imOfReal, layout, temp), '别家的 im（在听它自己的口）不算自己人')
  assert.ok(!proc.ownsPid('status', statusOfReal, layout, temp), '别家的状态口不算自己人')
  assert.ok(!proc.ownsPid('config', configOfReal, layout, temp), '别家的模型门不算自己人')

  // 本家那支：在听本家的口 ⇒ 自己人。
  const tempHouse = otherHome(new Map([[40001, new Set([8081])]]))
  assert.ok(proc.ownsPid('im', { pid: 40001, exePath: layout.imExe, cmdline: layout.imExe }, layout, tempHouse))
  // 谁都没听（刚起还没绑上/挂死）⇒ 端口这一维**不能否认**，由老判据说话（启动窗口不误判）。
  assert.ok(proc.ownsPid('im', { pid: 40002, exePath: layout.imExe, cmdline: layout.imExe }, layout, tempHouse))
  // 反方向：别家的那支（在听 8081）在真家（8080）眼里同样是外人 —— 判据对两家对称。
  const realHouse = homeOf({ listening: new Map([[40001, new Set([8081])]]) })
  assert.ok(!proc.ownsPid('im', { pid: 40001, exePath: layout.imExe, cmdline: layout.imExe }, layout, realHouse))
})

procCase('缺家事实 ⇒ **报错**（不许静默退回"只看可执行件" —— 那正是本刀堵的洞）', () => {
  const layout = { imExe: 'x\\celira-im.exe', hostExe: 'x\\agent_host.exe', clientDir: 'x\\client', repo: 'x' }
  // 五支各自缺自己的那一项。
  assert.throws(() => proc.ownsPid('agent', { exePath: layout.hostExe, cmdline: 'a' }, layout, {}), /家事实缺/)
  assert.throws(() => proc.ownsPid('client', { name: 'node.exe', cmdline: 'serve_client.mjs' }, layout, {}), /家事实缺/)
  assert.throws(() => proc.ownsPid('im', { exePath: layout.imExe }, layout, {}), /家事实缺/)
  assert.throws(() => proc.ownsPid('status', { name: 'node.exe', cmdline: 'home.js status --serve' }, layout, {}), /家事实缺/)
  assert.throws(() => proc.ownsPid('config', { name: 'node.exe', cmdline: 'home.js config --serve' }, layout, {}), /家事实缺/)
  // 整份家事实都不给：同样炸（不是"都当外人"）。
  assert.throws(() => proc.ownsPid('agent', { exePath: layout.hostExe, cmdline: 'a' }, layout, undefined), /家事实缺/)
  // 端口维还要求快照：给了端口、没给 `listening` ⇒ 炸。
  assert.throws(
    () => proc.ownsPid('im', { pid: 1, exePath: layout.imExe }, layout, { imPort: '8080' }),
    /端口快照/,
  )
  // 正对照：事实齐了就不抛（哪怕是"不是我们的"这种正常否定）。
  assert.equal(proc.ownsPid('im', { pid: 1, exePath: 'other.exe' }, layout, homeOf()), false)
})

procCase('端口口径：`listenerOf` 的端口由家事实一处说了算（宿主不监听 ⇒ null）', () => {
  // agent 永远不监听端口（出站进程）：不做任何 I/O 就返回 null。
  assert.equal(proc.listenerOf('agent', { hostExe: 'x' }, homeOf()), null)
  // 未登记的 target 照样要炸（与 ownsPid 同一条纪律）。
  assert.throws(() => proc.listenerOf('nope', { hostExe: 'x' }, homeOf()), /未登记的进程 target/)
})

procCase('pid 归属：**未登记的 target 必须报错**，不许静默当外人（最坏的假绿）', () => {
  const layout = { imExe: 'x', hostExe: 'y', clientDir: 'z', repo: 'r' }
  const info = { name: 'node.exe', cmdline: 'node whatever.js' }
  assert.throws(() => proc.ownsPid('nope', info, layout, homeOf()), /未登记的进程 target/)
  assert.throws(() => proc.ownsPid(undefined, info, layout, homeOf()), /未登记的进程 target/)
  // 正对照：登记过的 target 不抛（哪怕是"不是我们的"这种正常否定）。
  assert.equal(proc.ownsPid('client', info, layout, homeOf()), false)
})

procCase('存活判据：自己是活的；0 与不存在的 pid 是死的', () => {
  assert.ok(proc.isAlive(process.pid))
  assert.ok(!proc.isAlive(0))
  assert.ok(!proc.isAlive(-1))
  assert.ok(!proc.isAlive(999999999))
})

// ── 2026-09-25：家事实的第二维（`--home`）——同包 + 同端口时唯一能分开两个家的东西 ──

procCase('flagValue：取 `--旗标 <值>`（成对取，别的参数里的同名词不算）', () => {
  assert.equal(proc.flagValue('node home.js status --serve --home D:\\h', '--home'), 'D:\\h')
  assert.equal(proc.flagValue('node home.js status --serve', '--home'), null)
  assert.equal(proc.flagValue('node x.js --home "C:\\a b"', '--home'), 'C:\\a b', '带空格的路径按引号切')
  assert.equal(proc.flagValue('node x.js --home', '--home'), null, '旗标在末尾没有取值 ⇒ 没有')
})

procCase('家事实第二维三态：没声明 ⇒ absent；声明且是本家 ⇒ ours；声明但不是 ⇒ foreign', () => {
  const home = homeOf()
  assert.equal(proc.homeFlagVerdict({ cmdline: 'node home.js status --serve' }, home), 'absent')
  assert.equal(
    proc.homeFlagVerdict({ cmdline: 'node home.js status --serve --home D:\\celira-home' }, home),
    'ours',
  )
  assert.equal(
    proc.homeFlagVerdict({ cmdline: 'node home.js status --serve --home F:\\celira-test\\home' }, home),
    'foreign',
  )
  if (proc.IS_WINDOWS) {
    // 大小写/斜杠方向无关（与 samePath 同一条判据）。
    assert.equal(
      proc.homeFlagVerdict({ cmdline: 'node home.js status --serve --home d:/CELIRA-home/' }, home),
      'ours',
    )
  } else {
    // POSIX：大小写敏感 —— 大小写不同就是**另一个家**（不许被读成 ours）；末尾斜杠归一。
    const posixHome = homeOf({ homeDir: '/srv/celira-home' })
    assert.equal(
      proc.homeFlagVerdict({ cmdline: 'node home.js status --serve --home /srv/celira-home/' }, posixHome),
      'ours',
    )
    assert.equal(
      proc.homeFlagVerdict({ cmdline: 'node home.js status --serve --home /srv/CELIRA-home' }, posixHome),
      'foreign',
    )
    // 精确 argv 形态（/proc 给的）：同一套判据照旧。
    assert.equal(
      proc.homeFlagVerdict({ argv: ['node', 'home.js', 'status', '--serve', '--home', '/srv/celira-home'] }, posixHome),
      'ours',
    )
  }
  // ⚠ 声明了家、而调用方没给 homeDir ⇒ **炸**（不许静默退回"只看端口"：那正是两家互认的入口）。
  assert.throws(
    () => proc.homeFlagVerdict({ cmdline: 'node home.js status --serve --home D:\\x' }, { imPort: '8080' }),
    /家事实缺 homeDir/,
  )
})

procCase('**回归（真机踩到的那一格）**：同包 + **同端口**的两家，靠 `--home` 分开', () => {
  // 真机读数（2026-09-25）：两家默认端口相同 ⇒ 端口维判 'mine'、可执行件路径也相同 ⇒
  // 本家把别家的状态口/配置门/世界认成自己的，`down` 于是可能停掉别家的东西。
  const layout = {
    imExe: 'F:\\celira-test\\bin\\celira-im.exe',
    hostExe: 'F:\\celira-test\\bin\\agent_host.exe',
    clientDir: 'F:\\celira-test\\client',
    repo: 'F:\\celira-test',
  }
  // 两家的端口**故意设成一样**（默认配置的复现），快照说 pid 40001 在听 5190。
  const snapshot = new Map([[40001, new Set([5190])]])
  const mine = homeOf({ statusPort: '5190', listening: snapshot })
  const theirs = otherHome(snapshot)
  theirs.statusPort = '5190'

  const theirDoor = {
    pid: 40001,
    name: 'node.exe',
    cmdline: 'node F:\\celira-test\\home.js status --serve --home F:\\celira-test\\home',
  }
  // `up` 起的常驻件带 `--home` ⇒ 铁证：不是本家的（端口一样也救不了它）。
  assert.equal(proc.ownsPid('status', theirDoor, layout, mine), false)
  assert.equal(proc.ownsPid('status', theirDoor, layout, theirs), true, '本家认自己那支')
  // 配置门同款。
  const theirConfig = {
    pid: 40001,
    name: 'node.exe',
    cmdline: 'node F:\\celira-test\\home.js config --serve --home F:\\celira-test\\home',
  }
  assert.equal(proc.ownsPid('config', theirConfig, layout, mine), false)
  // 发布形态的静态页同款（`serve_client.mjs … --port 5173 --home …`）。
  const theirPage = {
    pid: 40002,
    name: 'node.exe',
    cmdline: 'node F:\\celira-test\\scripts\\serve_client.mjs --dist d --port 5173 --home F:\\celira-test\\home',
  }
  assert.equal(proc.ownsPid('client', theirPage, layout, homeOf({ listening: new Map([[40002, new Set([5173])]]) })), false)

  // **如实登记的残留**：手工直敲的常驻件没有 `--home`（手册里的命令就是那样）⇒ 那一档只有
  // 端口这一维 ⇒ 同端口时仍会互认。这不是本次的漏洞，是"手工起"这条路本身的边界，写下来
  // 免得将来有人以为它已经解决了。
  const manualDoor = { pid: 40001, name: 'node.exe', cmdline: 'node F:\\celira-test\\home.js status --serve' }
  assert.equal(proc.ownsPid('status', manualDoor, layout, mine), true)
})

procCase('账本四态：没写/已退 ⇒ gone；活着但不是本家的人 ⇒ not-ours（**不许当"已经在跑"**）', () => {
  const fs = require('node:fs')
  const os = require('node:os')
  const path = require('node:path')
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-ledger-'))
  const pidFile = path.join(dir, 'im.pid')
  const layout = {
    imExe: 'D:\\celira\\target\\release\\celira-im.exe',
    hostExe: 'D:\\celira\\agent\\target\\release\\agent_host.exe',
    clientDir: 'D:\\celira\\client',
    repo: 'D:\\celira',
  }
  const home = homeOf()
  // 没写
  assert.deepEqual(proc.ledgerPid('im', pidFile, layout, home), { pid: null, state: 'gone' })
  // 写了个不存在的数
  fs.writeFileSync(pidFile, '999999999\n')
  assert.deepEqual(proc.ledgerPid('im', pidFile, layout, home), { pid: null, state: 'gone' })
  // 写了**我们自己的 node 进程**的 pid：活着，但它不是 celira-im ⇒ not-ours。
  // 这就是"陈旧账本撞上无关进程"的那一档——旧实现（只看活着）会误报"已经在跑"。
  fs.writeFileSync(pidFile, `${process.pid}\n`)
  const ledger = proc.ledgerPid('im', pidFile, layout, home)
  assert.equal(ledger.pid, process.pid)
  assert.equal(ledger.state, 'not-ours')
  assert.equal(proc.trustedLedgerPid('im', pidFile, layout, home), null)
})

procCase('账本·家事实要传到位：缺了 ⇒ 报错（账本这条路不许静默退回老判据）', () => {
  // 为什么单独钉这一条：账本判据是 `down` 杀人的前置 —— 它要是静默退回"只看可执行件"，
  // 本刀修的洞会从这条缝里原样回来（别家账本 ⇒ 误杀）。去掉家校验，本用例必须红。
  const fs = require('node:fs')
  const os = require('node:os')
  const path = require('node:path')
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-ledger-cross-'))
  const pidFile = path.join(dir, 'host.pid')
  const layout = { hostExe: 'F:\\celira-test\\bin\\agent_host.exe', imExe: 'F:\\celira-test\\bin\\celira-im.exe' }
  // 借我们自己的进程当"活着的那支"（进程表查得到）：判据要真的走到家事实那一步。
  fs.writeFileSync(pidFile, `${process.pid}\n`)
  assert.throws(() => proc.ledgerPid('agent', pidFile, layout, undefined), /家事实缺/)
  assert.throws(() => proc.trustedLedgerPid('agent', pidFile, layout, { envDir: '' }), /家事实缺/)
  assert.throws(() => proc.ledgerPid('im', pidFile, layout, { ...otherHome(), imPort: undefined }), /家事实缺/, 'im 走端口维，缺 imPort 也要炸')
  // 正对照：家事实齐了 ⇒ 正常给出读数（这里是个不是我们的进程 ⇒ not-ours ⇒ down 只清账本不杀）。
  assert.equal(proc.ledgerPid('agent', pidFile, layout, otherHome()).state, 'not-ours')
  assert.equal(proc.trustedLedgerPid('agent', pidFile, layout, otherHome()), null)
})

// ── 2026-09-30（Linux 第一刀）：POSIX 的读法与"读不到 ≠ 不在" ──────────────

procCase('ss 解析（Linux 的端口→pid）：端口取本地地址末段；pid 一个不落；**看不到 pid 也如实产出**', () => {
  const output = [
    'State  Recv-Q Send-Q Local Address:Port  Peer Address:PortProcess',
    'LISTEN 0      128          0.0.0.0:8644         0.0.0.0:*    users:(("hermes",pid=2385510,fd=40))',
    'LISTEN 0      4096   127.0.0.53%lo:53         0.0.0.0:*        ',
    'LISTEN 0      511        [::1]:5190           [::]:*      users:(("node",pid=123,fd=20),("node",pid=124,fd=21))',
    'ESTAB  0      0      127.0.0.1:8080       127.0.0.1:51209',
    'LISTEN 0      511        127.0.0.1:18080      0.0.0.0:*   users:(("node",pid=999,fd=1))',
  ].join('\n')
  assert.deepEqual(proc.parseSsListeners(output), [
    { pid: 2385510, port: 8644 },
    { pid: null, port: 53 }, // 别人的进程：**知道有人在听、看不到是谁** —— 与"没有"是两件事
    { pid: 123, port: 5190 },
    { pid: 124, port: 5190 },
    { pid: 999, port: 18080 },
  ])
  // 认不出形状的行不许猜（头行、ESTAB、半截行）。
  assert.deepEqual(proc.parseSsListeners('LISTEN 0 1'), [])
  assert.deepEqual(proc.parseSsListeners(''), [])
})

procCase('真机：describeProcess 读本进程的**真实路径**与命令行（POSIX：/proc；名字不是 comm 的线程名）', () => {
  const info = proc.describeProcess(process.pid)
  assert.ok(info, '本进程应当读得到')
  assert.equal(info.pid, process.pid)
  if (!proc.IS_WINDOWS) {
    const fs = require('node:fs')
    const path = require('node:path')
    assert.ok(info.exePath, 'exe 的真实路径必须读得到（"按路径核验"的前提）')
    assert.equal(fs.realpathSync(info.exePath), info.exePath, 'exe 路径应当是解析过的真实路径')
    assert.equal(info.name, path.basename(info.exePath), '名字应当是**可执行件末段**（不是 comm 的线程名）')
    assert.ok(Array.isArray(info.argv) && info.argv.length > 0, 'argv 必须读得到（精确切词的来源）')
    assert.equal(info.cmdline, info.argv.join(' '))
  }
})

procCase('真机：按可执行件末段查进程表（POSIX 扫 /proc）—— 本进程在列；读不到 exe 的进程不冒充', () => {
  if (proc.IS_WINDOWS) {
    console.log('      （跳过：这一条钉的是 POSIX 的 /proc 读法）')
    return
  }
  const path = require('node:path')
  const rows = proc.listProcessesByName(path.basename(process.execPath))
  assert.ok(
    rows.some((row) => row.pid === process.pid),
    `进程表里应当有本进程（按可执行件末段找）`,
  )
  // 别人的进程（root 的）：exe 读不到 ⇒ **不列出来**（宁可不列，也不按名字冒充自己人）。
  if (typeof process.getuid === 'function' && process.getuid() !== 0) {
    const rootRow = proc.describeProcess(1)
    if (rootRow) {
      assert.equal(rootRow.exePath, null, 'root 的 exe 读不到 —— 那是"读不到"，不是"不在"（行还在，cmdline 也读得到）')
      assert.ok(!rows.some((row) => row.pid === 1), '读不到 exe 的进程不许进列表')
    }
  }
})

procAsyncCase('真机：真起一个监听套接字 ⇒ 端口→pid、快照、占用判据都读得到（关掉后如实放行）', async () => {
  const net = require('node:net')
  const server = net.createServer()
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  try {
    const pids = proc.listenersOnPort(port)
    assert.ok(pids.includes(process.pid), `端口 ${port} 上应当读得到本进程（读到的是 ${JSON.stringify(pids)}）`)
    assert.ok(proc.listenerSnapshot().get(process.pid)?.has(port), '快照里本进程要挂着这个端口')
    assert.equal(proc.portInUse(port), true)
  } finally {
    await new Promise((resolve) => server.close(resolve))
  }
  // 关掉之后：**等它真的放掉**再判（LISTEN 套接字的释放是异步的）——最多 3 秒。
  for (let attempt = 0; attempt < 15; attempt += 1) {
    if (!proc.portInUse(port)) break
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
  assert.equal(proc.portInUse(port), false, '关掉之后端口应当放掉')
})

procAsyncCase('真机：killPid 等它**真的退场**才回话（不是发完信号就说停好了）', async () => {
  const { spawn } = require('node:child_process')
  const child = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 60000)'], {
    detached: true,
    stdio: 'ignore',
  })
  child.unref()
  try {
    await new Promise((resolve) => setTimeout(resolve, 300))
    assert.ok(proc.isAlive(child.pid), '探针进程应当活着（否则这条用例什么也没验）')
    const stopped = await proc.killPid(child.pid, { force: true })
    assert.equal(stopped, true)
    assert.equal(proc.isAlive(child.pid), false, 'killPid 回话时它必须已经退场')
  } finally {
    if (proc.isAlive(child.pid)) {
      try {
        process.kill(child.pid, 'SIGKILL')
      } catch {
        // 已经没了
      }
    }
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 17, `用例数 ${cases} 少于声明的 17`)
})
