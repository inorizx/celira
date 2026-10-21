#!/usr/bin/env node
/**
 * **发布渠道探针**：这条渠道能不能回答"更新"需要的那三个问题？
 *
 *  ① 最新是哪个版本？ ② 那一版的产物在哪（下载地址）？ ③ 那个产物的哈希是多少？
 *
 * 为什么要有它：`PROJECT_SHAPE.md` §7 缺 6「发布渠道未裁」——今天"从哪知道有新版本"没有落点，
 * 而"更新"这件事的全部前提就是这三个问题有人答得上。这条探针**只量渠道**，不实现更新：
 * 它把答案按**普通用户会看到的样子**打出来（那几行就是壳将来要在「关于」页里显示的东西），
 * 答不上的那一格**点名**，绝不用"应该没事"糊过去。
 *
 * ## 三种渠道
 *
 * - `--channel gitee`（缺省）：真的去问 Gitee 的 OpenAPI v5（`releases` 列表，按 `created_at` 取最新）。
 *   ⚠ **用列表端点而不是某个 `latest` 端点**：列表是确定存在的那个；"最新"由我们按时间排出来，
 *   少依赖一个可能不存在的动词。有 `GITEE_TOKEN` 就带上（私有仓库/限流时需要）。
 * - `--channel github`（2026-10-21 加）：两页 HTML，**不带 token、不耗 API 额度、不需要 gh**——
 *   ① `releases.atom` 拿最新那一版的 tag；
 *   ② `releases/expanded_assets/<tag>` 拿这一版**真实挂着**的附件（名字、下载地址、页面上印着的
 *     `sha256:`），**不按命名约定去猜 URL**：猜出来的地址答不上"它到底在不在"。
 *   ⚠ 为什么不走 `api.github.com`：匿名 60 次/小时、且按出口 IP 算（实测被限过）——一条给普通用户
 *   点的"有没有新版"不该消耗那份额度。Gitee 那条没有 HTML 版可用，才走 OpenAPI。
 * - `--channel <文件或目录>`：读一份**离线夹具**。文件＝Gitee 那个列表的 JSON；目录里有
 *   `releases.atom` ⇒ 按 GitHub 形状读（配 `expanded_assets.html`），有 `releases.json` ⇒ 按 Gitee 形状读。
 *   用于演示与离线套件（**这条探针的判据要能在没有网络的机器上被证明**）。
 *
 * ## 判据（退出码）
 *
 * - **0** ＝ 渠道把三格都答上了（版本 ＋ 产物地址 ＋ 哈希），且它们互相对得上；
 * - **2** ＝ **查不到**：匿名读不到那个仓库（私有/未建）、没有发行版、或网络不通 —— **不适用，
 *   既不算通过也不算失败**（但它是一条**结论**：这条渠道今天还不能当更新源）；
 * - **3** ＝ 答了，但形状读不懂（缺字段 —— 点名缺哪一格，那说明我们对着的 API 版本不对）；
 * - **4** ＝ 用法错（`--current` 与 `--package` 都没给 / 都不对）。
 *
 * ⚠ **它不会替你判的那一处**：产物地址与哈希**是不是**真的对应那一版的字节。要证明那件事，
 * 得把产物下下来算一遍 —— 那是"自己换"那一格落地时的判据（届时补一条真机门禁），不在这条探针里。
 */
import fs from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { execFileSync } from 'node:child_process'

const GITEE_API = 'https://gitee.com/api/v5'
const EXIT = { OK: 0, USAGE: 1, NOT_APPLICABLE: 2, SHAPE: 3, CANNOT_TELL: 4 }

/**
 * 剥掉 UTF-8 BOM。
 *
 * 为什么要管这件事：Windows 上 `Set-Content -Encoding utf8`（PowerShell 5.1）**写 BOM**，而
 * `JSON.parse` 不认它（`Unexpected token '﻿'`）—— 演示用的夹具就是这么被噎住的（实测）。
 * 网络回信不会有 BOM，但**夹具是人手写的**，所以夹具这条路上必须剥。
 */
function stripBom(text) {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text
}

/** 从 `<包目录>/VERSION` 读出这一份在跑的版本（`version = 0.1.0` 那一行）。 */
export function versionFromPackage(packageDir) {
  const file = path.join(packageDir, 'VERSION')
  if (!fs.existsSync(file)) return { ok: false, why: `没有 VERSION：${file}` }
  const line = stripBom(fs.readFileSync(file, 'utf8'))
    .split(/\r?\n/)
    .find((each) => /^\s*version\s*=/.test(each))
  if (!line) return { ok: false, why: `VERSION 里没有 version 那一行：${file}` }
  return { ok: true, version: line.split('=')[1].trim() }
}

/** `1.2.3` / `0.1.0-dirty` 这种号：取数字段比较；够用就够用，不发明 semver 库。 */
export function compareVersions(a, b) {
  const parts = (value) => String(value).split(/[.\-+]/).map((each) => (/^\d+$/.test(each) ? Number(each) : each))
  const left = parts(a)
  const right = parts(b)
  for (let i = 0; i < Math.max(left.length, right.length); i += 1) {
    const x = left[i]
    const y = right[i]
    if (x === undefined) return -1
    if (y === undefined) return 1
    if (typeof x === 'number' && typeof y === 'number') {
      if (x !== y) return x < y ? -1 : 1
      continue
    }
    const sx = String(x)
    const sy = String(y)
    if (sx !== sy) return sx < sy ? -1 : 1
  }
  return 0
}

function tagVersion(release) {
  const raw = String(release.tag_name ?? release.name ?? '').trim()
  return raw.replace(/^v/i, '')
}

function assetOf(release) {
  const assets = Array.isArray(release.assets) ? release.assets : []
  if (assets.length === 0) return { ok: false, why: '这一版没有附件' }
  // 默认下载那个包：all-in-one。名字对不上就如实说"没找到默认下载"（不猜别的那一个）。
  const wanted = assets.find((each) => /^celira-\d/.test(String(each.name ?? ''))) ?? assets[0]
  const url = wanted.browser_download_url ?? wanted.download_url ?? null
  if (!url) return { ok: false, why: `附件 ${wanted.name ?? '(无名)'} 没有下载地址字段` }
  return { ok: true, name: wanted.name ?? '(无名)', url, size: wanted.size ?? null }
}

/** 渠道答的三格 —— **纯函数**（用例直接量它，不碰网络）。 */
export function readChannel(releases) {
  if (!Array.isArray(releases)) return { ok: false, notApplicable: false, why: '渠道回的不是一个列表' }
  if (releases.length === 0) return { ok: false, notApplicable: true, why: '这个仓库还没有任何发行版' }
  const newest = [...releases].sort((a, b) => String(b.created_at ?? '').localeCompare(String(a.created_at ?? '')))[0]
  const version = tagVersion(newest)
  if (!version) return { ok: false, notApplicable: false, why: '最新的那一版没有 tag/name，读不出版本号' }
  const asset = assetOf(newest)
  const entry = {
    version,
    tag: newest.tag_name ?? null,
    created: newest.created_at ?? null,
    asset: asset.ok ? { name: asset.name, url: asset.url, size: asset.size } : null,
  }
  if (!asset.ok) {
    return { ok: false, notApplicable: false, why: asset.why, partial: entry }
  }
  return { ok: true, entry }
}

/**
 * atom 里**最新那一版**是谁：`<link href="…/releases/tag/<tag>">` ＋ `<updated>`。
 *
 * 取第一条 entry：GitHub 的 atom 是按时间倒序的，"最新"由它替我们排（比 Gitee 那条更省事）。
 */
export function readGithubTag(atom) {
  const entry = /<entry>([\s\S]*?)<\/entry>/.exec(String(atom ?? ''))
  if (!entry) {
    return { ok: false, notApplicable: true, why: 'atom 里没有任何 entry（这个仓库还没有发行版）' }
  }
  const link = /<link[^>]*href="([^"]+)"/.exec(entry[1])
  if (!link) return { ok: false, notApplicable: false, why: '最新那条没有 <link>，读不出版本号' }
  const tagMatch = /\/releases\/tag\/([^/"]+)/.exec(link[1])
  if (!tagMatch) return { ok: false, notApplicable: false, why: `链接不是 /releases/tag/<tag> 形状：${link[1]}` }
  const updated = /<updated>([^<]+)<\/updated>/.exec(entry[1])
  const tag = decodeURIComponent(tagMatch[1])
  return { ok: true, tag, version: tag.replace(/^v/i, ''), created: updated ? updated[1].trim() : null }
}

/**
 * **GitHub 形状的两页**：atom（哪一版最新）＋ 附件页（这一版挂了什么）。
 *
 * 纯函数：两页 HTML 进，三格出 —— 用例直接量它，不碰网络。判据与 Gitee 那条同源：
 * 版本、产物地址、产物身份。差别的只有**身份那一格**：GitHub 的附件页印着 `sha256:`，
 * 所以这一格答得出哈希；答不出就是答不出（不拿大小冒充哈希）。
 */
export function readGithubRelease({ atom, assetsHtml, repo = null } = {}) {
  const head = readGithubTag(atom)
  if (!head.ok) return head
  // 附件页每一行一个附件（`<li class="Box-row …">`）：行里同时有下载链接与（新版 UI 的）`sha256:`。
  // 只认 `releases/download/…`：源码那两条（`archive/refs/tags/…`）不是发布产物。
  const assets = []
  for (const row of String(assetsHtml ?? '').split(/<li\b/).slice(1)) {
    const href = /href="(\/[^"]*\/releases\/download\/[^"]+)"/.exec(row)
    if (!href) continue
    const digest = /sha256:([0-9a-fA-F]{64})/.exec(row)
    assets.push({
      name: decodeURIComponent(href[1].split('/').pop()),
      url: `https://github.com${href[1]}`,
      size: null,
      hash: digest ? digest[1].toLowerCase() : null,
    })
  }
  if (assets.length === 0) {
    return {
      ok: false,
      notApplicable: false,
      why: `这一版没有挂任何产物（${head.tag} 只有源码压缩包）`,
      partial: { version: head.version, tag: head.tag, created: head.created, asset: null },
    }
  }
  // 默认下载那个包：all-in-one。名字对不上就如实说"按第一个来"（不猜别的那一个）。
  const wanted = assets.find((each) => /^celira-\d.*\.zip$/i.test(each.name)) ?? assets[0]
  return {
    ok: true,
    entry: {
      version: head.version,
      tag: head.tag,
      created: head.created,
      repo,
      asset: { name: wanted.name, url: wanted.url, size: wanted.size },
      hash: wanted.hash,
    },
  }
}

/** 问 GitHub（真网，两页 HTML；不带 token、不耗 API 额度、不需要 gh）。`repo` 形如 `owner/name`。 */
async function fetchGithub(repo) {
  const atom = await fetchText(`https://github.com/${repo}/releases.atom`, 'text/html,application/xhtml+xml')
  if (!atom.ok) {
    if (atom.status === 404) {
      return { ok: false, notApplicable: true, why: `404：匿名读不到 ${repo}（仓库还没公开，或名字不对）` }
    }
    if (atom.status) return { ok: false, notApplicable: true, why: `atom HTTP ${atom.status}` }
    return { ok: false, notApplicable: true, why: `连不上 GitHub：${atom.why}` }
  }
  const head = readGithubTag(atom.text)
  if (!head.ok) return head

  // 第二页只问**这一版**的附件：拿不到就交给 readGithubRelease 如实说"没挂产物"（不猜 URL）。
  let assetsHtml = ''
  const page = await fetchText(
    `https://github.com/${repo}/releases/expanded_assets/${encodeURIComponent(head.tag)}`,
    'text/html',
  )
  if (page.ok) assetsHtml = page.text
  return readGithubRelease({ atom: atom.text, assetsHtml, repo })
}

/** 问 Gitee（真网）。`repo` 形如 `owner/name`。 */
async function fetchGitee(repo, token) {
  const url = `${GITEE_API}/repos/${repo}/releases?per_page=5${token ? `&access_token=${encodeURIComponent(token)}` : ''}`
  let text = null
  let status = null
  if (token) {
    // ⚠ 带 token 时**只走进程内 fetch**，不走 curl：curl 那条路会把 URL（连同 token）放进命令行，
    // 同一台机器上任何进程都读得到 argv。这台机器直连不通时，宁如实说"连不上"，也不把凭据交出去。
    try {
      const response = await fetch(url, { headers: { accept: 'application/json' }, signal: AbortSignal.timeout(20000) })
      status = response.status
      if (response.ok) text = await response.text()
    } catch (error) {
      const cause = error && error.cause ? ` (${error.cause.code ?? error.cause.message})` : ''
      return {
        ok: false,
        notApplicable: true,
        why: `带 GITEE_TOKEN 时只走进程内 fetch（curl 会把 token 放进命令行）；这台机器直连不通：${error.message}${cause}`,
      }
    }
  } else {
    const answer = await fetchText(url, 'application/json')
    if (!answer.ok) {
      if (answer.status === 404) {
        return {
          ok: false,
          notApplicable: true,
          why: `404：匿名读不到 ${repo}（仓库还没公开，或还没建发行版）—— 普通用户不填 token 就更新不了`,
        }
      }
      if (answer.status) return { ok: false, notApplicable: true, why: `HTTP ${answer.status}` }
      return { ok: false, notApplicable: true, why: `连不上 Gitee：${answer.why}` }
    }
    text = answer.text
  }
  if (status === 404) {
    return {
      ok: false,
      notApplicable: true,
      why: `404：匿名读不到 ${repo}（仓库还没公开，或还没建发行版）—— 普通用户不填 token 就更新不了`,
    }
  }
  let body
  try {
    body = JSON.parse(text)
  } catch (error) {
    return { ok: false, notApplicable: false, why: `回信不是 JSON：${error.message}` }
  }
  return { ...readChannel(body), raw: body }
}

/**
 * 读离线夹具。两种形状，按**目录里有什么文件**判（不按扩展名猜）：
 * - 有 `releases.atom` ⇒ GitHub 形状（配 `expanded_assets.html`）；
 * - 否则读 `releases.json`（Gitee 那个列表的形状；直接给一个文件也走这一支）。
 *
 * 为什么要有 GitHub 形状的夹具：`--channel github` 走真网，可**判据必须能在没有网络的机器上
 * 被证明**——两页 HTML 存下来就是它的夹具（2026-10-21 加，跟着 GitHub 渠道一起）。
 */
function readFixture(target) {
  const isDir = fs.statSync(target).isDirectory()
  const atomFile = path.join(target, 'releases.atom')
  if (isDir && fs.existsSync(atomFile)) {
    const assetsFile = path.join(target, 'expanded_assets.html')
    return readGithubRelease({
      atom: stripBom(fs.readFileSync(atomFile, 'utf8')),
      assetsHtml: fs.existsSync(assetsFile) ? stripBom(fs.readFileSync(assetsFile, 'utf8')) : '',
    })
  }
  const file = isDir ? path.join(target, 'releases.json') : target
  if (!fs.existsSync(file)) return { ok: false, notApplicable: false, why: `夹具不在：${file}` }
  let body
  try {
    body = JSON.parse(stripBom(fs.readFileSync(file, 'utf8')))
  } catch (error) {
    return { ok: false, notApplicable: false, why: `夹具不是 JSON：${error.message}` }
  }
  return { ...readChannel(body), raw: body }
}

function repoFromOrigin() {
  try {
    const url = execFileSync('git', ['remote', 'get-url', 'origin'], { encoding: 'utf8' }).trim()
    const match = /(?:gitee\.com|github\.com)[:/]([^/]+)\/([^/]+?)(?:\.git)?$/.exec(url)
    return match ? `${match[1]}/${match[2]}` : null
  } catch {
    return null
  }
}

/**
 * **拿一页 HTML 的两条路**（2026-10-21 加，实测逼出来的）。
 *
 * 这台机器直连 github.com 是**连不上的**（`UND_ERR_CONNECT_TIMEOUT`），而环境里配着代理
 * （`HTTPS_PROXY`）——Node 的 `fetch` **不认**这些变量（22 上如此；`NODE_USE_ENV_PROXY` 在这个
 * 版本上也不生效），`curl` 认。于是"这里说连不上"和"其实网是通的"会同时为真。
 *
 * 判据按顺序、各有理由：
 * - **环境里有代理 ⇒ 先 curl**：Node 这侧忽略代理，先试 fetch 等于先花一次 20 秒超时；
 * - **没有代理 ⇒ 先 fetch**：那是零子进程的路；
 * - 两条都失败才叫失败，**两条的原因都报出来**（"连不上"必须能解释成哪一种连不上）。
 *
 * 为什么不自己写 CONNECT 隧道：这只手要的是"拿两页 HTML"，curl 在 Windows 10+ / Linux / macOS
 * 上都在，而一段手写的 chunked 解码是更容易错的东西。curl 也没有时，就如实说没网。
 */
function proxyConfigured() {
  for (const name of ['HTTPS_PROXY', 'https_proxy', 'ALL_PROXY', 'all_proxy']) {
    const value = process.env[name]
    if (typeof value === 'string' && value.trim() !== '') return value.trim()
  }
  return null
}

function curlText(url) {
  try {
    const text = execFileSync('curl', ['-sS', '-L', '--max-time', '25', '-A', 'celira-probe/1', url], {
      encoding: 'utf8',
      maxBuffer: 16 * 1024 * 1024,
      windowsHide: true,
    })
    return { ok: true, text }
  } catch (error) {
    const detail = String(error.stderr ?? error.message ?? error).trim().split(/\r?\n/)[0]
    return { ok: false, why: `curl 那条路也失败：${detail}` }
  }
}

async function fetchText(url, accept) {
  const fetchOnce = async () => {
    try {
      const response = await fetch(url, { headers: { accept }, signal: AbortSignal.timeout(20000) })
      if (response.status === 404) return { ok: false, status: 404 }
      if (!response.ok) return { ok: false, status: response.status }
      return { ok: true, text: await response.text(), via: 'fetch' }
    } catch (error) {
      const cause = error && error.cause ? ` (${error.cause.code ?? error.cause.message})` : ''
      return { ok: false, why: `${error.message}${cause}` }
    }
  }
  const proxy = proxyConfigured()
  const order = proxy ? ['curl', 'fetch'] : ['fetch', 'curl']
  const why = []
  for (const route of order) {
    if (route === 'curl') {
      const answer = curlText(url)
      if (answer.ok) return answer
      why.push(answer.why)
    } else {
      const answer = await fetchOnce()
      if (answer.ok) return answer
      if (answer.status === 404) return answer
      why.push(`fetch: ${answer.why}`)
    }
  }
  return { ok: false, why: `${why.join('；')}${proxy ? `（环境里有代理 ${proxy}）` : '（环境里没有代理变量）'}` }
}

function parseArgs(argv) {
  const out = { channel: 'gitee', repo: null, current: null, packageDir: null, json: false, error: null }
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]
    const value = argv[i + 1] ?? ''
    if (arg === '--channel') {
      if (!value.trim() || value.startsWith('--')) {
        out.error = '--channel 要一个值（`gitee` / `github` / 一份夹具的路径）'
        return out
      }
      out.channel = value.trim()
      i += 1
    } else if (arg === '--repo') {
      if (!value.trim() || value.startsWith('--')) {
        out.error = '--repo 要 `owner/name`'
        return out
      }
      out.repo = value.trim()
      i += 1
    } else if (arg === '--current') {
      if (!value.trim() || value.startsWith('--')) {
        out.error = '--current 要一个版本号'
        return out
      }
      out.current = value.trim()
      i += 1
    } else if (arg === '--package') {
      if (!value.trim() || value.startsWith('--')) {
        out.error = '--package 要一个包目录（读它的 VERSION）'
        return out
      }
      out.packageDir = value.trim()
      i += 1
    } else if (arg === '--json') out.json = true
    else {
      out.error = `不认识的参数：${arg}`
      return out
    }
  }
  return out
}

async function main(argv) {
  const options = parseArgs(argv)
  if (options.error) {
    console.log(`[probe-release-channel] ${options.error}`)
    console.log('用法: node scripts/probe-release-channel.mjs [--package <包目录> | --current <版本>]')
    console.log('                                             [--channel gitee|github|<夹具路径>] [--repo owner/name] [--json]')
    return EXIT.USAGE
  }

  let current = options.current
  if (!current && options.packageDir) {
    const read = versionFromPackage(options.packageDir)
    if (!read.ok) {
      console.log(`[probe-release-channel] 读不到在跑的版本：${read.why}`)
      return EXIT.CANNOT_TELL
    }
    current = read.version
  }
  if (!current) {
    console.log('[probe-release-channel] 要给 `--current <版本>` 或 `--package <包目录>`（"你在跑哪一版"是这条探针的一半）')
    return EXIT.USAGE
  }

  const repo = options.repo ?? repoFromOrigin() ?? '(不知道)'
  console.log(`[app]     你在跑 ${current}`)

  let answer
  if (options.channel === 'gitee' || options.channel === 'github') {
    const github = options.channel === 'github'
    console.log(
      github
        ? `[channel] github → ${repo}（atom ＋ 附件页；不带 token、不耗 API 额度）`
        : `[channel] gitee → ${repo}${process.env.GITEE_TOKEN ? '（带 token）' : '（匿名）'}`,
    )
    if (repo === '(不知道)') {
      console.log('[channel] 问不出仓库：origin 不是 gitee/github，也没给 --repo')
      return EXIT.NOT_APPLICABLE
    }
    if (!github && /^[^/]+\/[^/]+$/.test(repo) && !process.env.GITEE_TOKEN) {
      // 仓库在 GitHub、问的却是 Gitee ⇒ 那条大概率 404。不是错，但这句话省一次猜。
      try {
        const origin = execFileSync('git', ['remote', 'get-url', 'origin'], { encoding: 'utf8' }).trim()
        if (/github\.com/.test(origin)) console.log('[channel] 注意：origin 在 github.com，这条问的是 Gitee —— 要问 GitHub 请加 `--channel github`')
      } catch {
        /* 不是 git 仓库：这句话就不说 */
      }
    }
    answer = github ? await fetchGithub(repo) : await fetchGitee(repo, process.env.GITEE_TOKEN ?? '')
  } else {
    const target = path.resolve(options.channel)
    console.log(`[channel] 夹具 → ${target}`)
    answer = readFixture(target)
  }

  if (!answer.ok) {
    const label = answer.notApplicable ? '查不到（不适用，不是失败）' : '形状读不懂'
    console.log(`[channel] ${label}：${answer.why}`)
    if (answer.partial) {
      console.log(`[channel] 它答上的那几格：版本 ${answer.partial.version}；产物格缺`)
    }
    if (options.json) console.log(JSON.stringify({ current, repo, channel: options.channel, latest: null, hasUpdate: null, answer }))
    return answer.notApplicable ? EXIT.NOT_APPLICABLE : EXIT.SHAPE
  }

  const { version, tag, created, asset, hash } = answer.entry
  console.log(`[channel] 最新发行版：${version}（${tag ?? '无 tag'}，${created ?? '无时间'}）`)
  console.log(`[channel] 产物：${asset.name}${asset.size ? `（${asset.size} 字节）` : ''}`)
  console.log(`[channel]          ${asset.url}`)
  // 第三格是**产物身份**：有哈希就报哈希（能拿来校字节），没有就如实说只有大小/地址。
  if (hash) console.log(`[channel] 产物身份：sha256 ${hash}`)
  else console.log(`[channel] 产物身份：渠道没给哈希${asset.size ? '（只有大小）' : '（只有一个地址）'}—— 更新器只能靠人核`)
  const order = compareVersions(current, version)
  if (order < 0) {
    console.log(`[update]  有新版：${current} → ${version}`)
    console.log('[update]  换法：下载 → 解压到新文件夹 → 托盘退出 → 双击新的那份（家不动）')
    console.log('[update]        有 scripts/update.ps1 就走它：同一条路，另加 sha256 校验与自动备份')
  } else if (order === 0) {
    console.log('[update]  已经是最新')
  } else {
    console.log(`[update]  你跑的比渠道上的还新（${current} > ${version}）—— 渠道上是旧版，如实说`)
  }
  console.log(
    `[probe-release-channel] 三格（版本 / 产物地址 / 产物身份）：${hash || asset.size ? '渠道答上了' : '只答上两格'}` +
      `${hash ? '，哈希可校字节' : '，没有哈希 —— 字节只能靠人核'}`,
  )
  console.log('[probe-release-channel] ⚠ 没量的那一处：产物地址里的字节**是不是**那一版（要下下来算，见文件头）')
  // `--json` 那一行是**给脚本读的**（update.ps1 直接吃它），所以是单行：谁来解析都不该猜缩进。
  // `hasUpdate` 由本文件自己的 compareVersions 算，消费方不再各写一份版本比较。
  if (options.json) {
    console.log(
      JSON.stringify({ current, repo, channel: options.channel, latest: version, hasUpdate: order < 0, answer }),
    )
  }
  return EXIT.OK
}

// ⚠ 别手拼 `file://`：Windows 上 `process.argv[1]` 是 `D:\...`，拼出来的 URL 与 `import.meta.url`
// 对不上（前缀与百分号编码两处都不一样）⇒ `main()` 静默不跑、退出码 0、一个字都不打（实测）。
const isMain = Boolean(process.argv[1]) && import.meta.url === pathToFileURL(process.argv[1]).href
if (isMain) {
  main(process.argv.slice(2))
    .then((code) => process.exit(code))
    .catch((error) => {
      console.error(`[probe-release-channel] 跑挂了：${error && error.stack ? error.stack : error}`)
      process.exit(1)
    })
}
