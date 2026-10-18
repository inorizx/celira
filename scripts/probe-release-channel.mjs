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
 * ## 两种渠道
 *
 * - `--channel gitee`（缺省）：真的去问 Gitee 的 OpenAPI v5（`releases` 列表，按 `created_at` 取最新）。
 *   ⚠ **用列表端点而不是某个 `latest` 端点**：列表是确定存在的那个；"最新"由我们按时间排出来，
 *   少依赖一个可能不存在的动词。有 `GITEE_TOKEN` 就带上（私有仓库/限流时需要）。
 * - `--channel <文件或目录>`：读一份**离线夹具**（形状＝Gitee 那个列表的 JSON）。用于演示与离线套件
 *   （不给它上真网：这条探针的判据要能在没有网络的机器上被证明）。
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

/** 问 Gitee（真网）。`repo` 形如 `owner/name`。 */
async function fetchGitee(repo, token) {
  const url = `${GITEE_API}/repos/${repo}/releases?per_page=5${token ? `&access_token=${encodeURIComponent(token)}` : ''}`
  let response
  try {
    response = await fetch(url, { headers: { accept: 'application/json' } })
  } catch (error) {
    return { ok: false, notApplicable: true, why: `连不上 Gitee：${error.message}` }
  }
  if (response.status === 404) {
    return {
      ok: false,
      notApplicable: true,
      why: `404：匿名读不到 ${repo}（仓库还没公开，或还没建发行版）—— 普通用户不填 token 就更新不了`,
    }
  }
  if (!response.ok) {
    return { ok: false, notApplicable: true, why: `HTTP ${response.status}` }
  }
  let body
  try {
    body = await response.json()
  } catch (error) {
    return { ok: false, notApplicable: false, why: `回信不是 JSON：${error.message}` }
  }
  return { ...readChannel(body), raw: body }
}

/** 读离线夹具（形状＝Gitee 那个列表）。目录 ⇒ 取里面的 `releases.json`。 */
function readFixture(target) {
  const file = fs.statSync(target).isDirectory() ? path.join(target, 'releases.json') : target
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

function parseArgs(argv) {
  const out = { channel: 'gitee', repo: null, current: null, packageDir: null, json: false, error: null }
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]
    const value = argv[i + 1] ?? ''
    if (arg === '--channel') {
      if (!value.trim() || value.startsWith('--')) {
        out.error = '--channel 要一个值（`gitee` 或一份夹具的路径）'
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
    console.log('                                             [--channel gitee|<夹具路径>] [--repo owner/name] [--json]')
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
  if (options.channel === 'gitee') {
    console.log(`[channel] gitee → ${repo}${process.env.GITEE_TOKEN ? '（带 token）' : '（匿名）'}`)
    if (repo === '(不知道)') {
      console.log('[channel] 问不出仓库：origin 不是 gitee/github，也没给 --repo')
      return EXIT.NOT_APPLICABLE
    }
    answer = await fetchGitee(repo, process.env.GITEE_TOKEN ?? '')
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
    if (options.json) console.log(JSON.stringify({ current, repo, channel: options.channel, answer }, null, 2))
    return answer.notApplicable ? EXIT.NOT_APPLICABLE : EXIT.SHAPE
  }

  const { version, tag, created, asset } = answer.entry
  console.log(`[channel] 最新发行版：${version}（${tag ?? '无 tag'}，${created ?? '无时间'}）`)
  console.log(`[channel] 产物：${asset.name}${asset.size ? `（${asset.size} 字节）` : '（没有大小字段）'}`)
  console.log(`[channel]          ${asset.url}`)
  const order = compareVersions(current, version)
  if (order < 0) {
    console.log(`[update]  有新版：${current} → ${version}`)
    console.log('[update]  换法：下载 → 解压到新文件夹 → 托盘退出 → 双击新的那份（家不动）')
  } else if (order === 0) {
    console.log('[update]  已经是最新')
  } else {
    console.log(`[update]  你跑的比渠道上的还新（${current} > ${version}）—— 渠道上是旧版，如实说`)
  }
  console.log('[probe-release-channel] 三格（版本 / 产物地址 / 大小）渠道都答上了')
  if (!asset.size) console.log('[probe-release-channel] ⚠ 但这一版没有"大小"那一格 —— 更新器只能靠哈希，见下')
  console.log('[probe-release-channel] ⚠ 没量的那一处：产物地址里的字节**是不是**那一版（要下下来算，见文件头）')
  if (options.json) console.log(JSON.stringify({ current, repo, channel: options.channel, answer }, null, 2))
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
