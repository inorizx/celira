// **预设供应商表**的纯逻辑测试（2026-09-23 预设供应商层）。
//
// 这一张表的危险性不在代码里，而在数据里：一条**只填到域名**的地址会让试连 404
// （TASK_77 踩过），一个**空泛的名字/简介**会让用户在左栏里无从选择。所以这里钉的
// 不是"函数返回什么"，而是**表里每一条自己成不成立**：
//
//  1. **地址是完整线协议地址**（末尾就是那一段），且与门那边的试连口径（摘尾拼 `/models`）相容；
//  2. **每家都有一句人话**（不是占位符）；
//  3. **「自定义」是最后一项**（裁决点名的形状，落在纯逻辑层）；
//  4. **预设 ≠ 已配置**：那句明示必须真的挂在左栏上（源级判据 —— 本仓没有 jsdom）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  CUSTOM_PRESET_ID,
  PRESET_DISCLAIMER,
  PROVIDER_PRESETS,
  groupEntriesByProvider,
  pickerRows,
  presetById,
  presetIdForEndpoint,
  providerLinks,
  providerRowState,
} from '../src/api/presets.ts'
import { probeUrlFor } from '../src/api/homeConfig.ts'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

/**
 * 门侧的 `modelsUrl`（试连要探的地址 = 摘掉线协议那一段再拼 `/models`）。
 *
 * 直接引门那份实现、**不照抄一份**：预设表与试连口径的相容性正是要判的东西，
 * 抄一份就只能证明"抄得像"（同源两侧的判据证明不了任何东西）。
 */
const { modelsUrl } = createRequire(import.meta.url)('../../scripts/home/config-server.js') as {
  modelsUrl: (endpoint: string) => string
}

test('预设表：十余家、id 唯一、每家都有一句人话（不是占位符）', () => {
  assert.ok(
    PROVIDER_PRESETS.length >= 12,
    `裁决要的是"主流供应商十余家"，实际 ${PROVIDER_PRESETS.length} 家`,
  )
  const ids = PROVIDER_PRESETS.map((preset) => preset.id)
  assert.equal(new Set(ids).size, ids.length, `id 不许重复：${ids.join(', ')}`)
  for (const preset of PROVIDER_PRESETS) {
    assert.ok(preset.name.trim().length > 0, `${preset.id} 没有名字`)
    assert.ok(preset.blurb.trim().length >= 4, `${preset.id} 的简介太短，不像一句能帮人挑的话`)
    assert.ok(!/todo|placeholder|测试用/i.test(preset.blurb), `${preset.id} 的简介是占位符`)
  }
})

test('★ 每条地址都是**完整线协议地址**，且与门的试连口径相容（TASK_77 的教训）', () => {
  for (const preset of PROVIDER_PRESETS) {
    // 不是合法绝对地址 ⇒ `new URL` 直接抛，用例红。
    const url = new URL(preset.endpointTemplate)
    assert.equal(url.protocol, 'https:', `${preset.id} 应当是 https：${preset.endpointTemplate}`)
    assert.match(
      url.pathname,
      /(\/chat)?\/completions$|\/responses$/,
      `${preset.id} 的地址末尾不是线协议那一段（只填到域名/基址会让试连 404）：${preset.endpointTemplate}`,
    )
    // 门那边的试连是 `GET <基址>/models`（把线协议那一段摘掉再拼）—— 预设表必须与它相容。
    const probed = modelsUrl(preset.endpointTemplate)
    assert.equal(
      new URL(probed).pathname.endsWith('/models'),
      true,
      `${preset.id} 的试连地址不是 …/models：${probed}`,
    )
    assert.ok(!probed.includes('/models/models'), `${preset.id} 摘尾没摘干净：${probed}`)
  }
})

test('presetById：找不到就是 `null`（**不回落第一条**，那是猜）', () => {
  assert.equal(presetById('deepseek')?.name, 'DeepSeek')
  assert.equal(presetById('nope'), null)
  assert.equal(presetById(null), null)
  assert.equal(presetById(undefined), null)
  assert.equal(presetById(''), null)
  assert.equal(presetById(CUSTOM_PRESET_ID), null, '「自定义」不是预设 —— 它没有地址模板')
})

test('★ 左栏行：预设在前、「自定义」是**最后一项**（裁决点名的形状）', () => {
  const rows = pickerRows()
  assert.equal(rows.length, PROVIDER_PRESETS.length + 1)
  const last = rows[rows.length - 1]!
  assert.equal(last.id, CUSTOM_PRESET_ID)
  assert.equal(last.name, '自定义')
  assert.deepEqual(
    rows.slice(0, -1).map((row) => row.id),
    PROVIDER_PRESETS.map((preset) => preset.id),
    '预设的顺序不许多出来/少一条',
  )
})

test('presetIdForEndpoint：「已保存」只按**地址相等**认，不是"这个供应商被配过"', () => {
  assert.equal(presetIdForEndpoint('https://api.deepseek.com/v1/chat/completions'), 'deepseek')
  assert.equal(
    presetIdForEndpoint('  https://api.deepseek.com/v1/chat/completions  '),
    'deepseek',
    '首尾空白不算差别',
  )
  assert.equal(presetIdForEndpoint('https://api.deepseek.com'), null, '只到基址不算：地址不同就是不同的一条')
  assert.equal(
    presetIdForEndpoint('https://my-own.example.com/v1/chat/completions'),
    null,
    '自己填的地址没有对应的预设（「自定义」本来就不在表里）',
  )
  assert.equal(presetIdForEndpoint(''), null)
})

test('★ 预设 ≠ 已配置：明示句在场，且**左栏真的把它渲染出来**（源级判据）', () => {
  assert.match(PRESET_DISCLAIMER, /不代表已经配好/, '明示句必须说清"选了预设不等于配好了"')
  assert.match(PRESET_DISCLAIMER, /钥匙/, '还要点明缺的是什么（钥匙）')
  // ⚠ 为什么是源级判据：本仓的 client 测试是 `node --test` + 类型剥离，**没有 jsdom /
  //    组件渲染**（与 `no-silent-red-path.test.ts` 同款处境）。判"这句明示上不上屏"
  //    在离线侧只能落到"左栏组件渲染了它"这一步；上屏本身由真机门禁那条断言守。
  //    判错即红的样子：把 `{PRESET_DISCLAIMER}` 删掉（明示句就只剩一个没人用的常量）。
  //
  // ⚠ 2026-09-23 二次回炉：左栏从 `PresetProviderPicker` 换成页面内的 `ProviderSidebar`
  //    （形状回到 2026-09-02 那版「点谁配谁」）。**判据的落点跟着搬，强度不变** ——
  //    它现在必须在**真的那个左栏**里被渲染出来。
  // ⚠ **2026-10-12 第三刀**：左栏又搬了一次 —— 从"页面内的 `ProviderSidebar`"搬进
  //    `components/celira/ProviderSidebar.tsx`（按主语拆：那一处只说"这份清单"）。
  //    **判据的落点跟着搬，强度不变**：它仍然必须在**真的那个左栏**里被渲染出来。
  const page = read('../src/components/celira/ProviderSidebar.tsx')
  assert.match(
    page,
    /function ProviderSidebar/,
    '左栏组件还叫得到（回炉后它搬进了页面里）',
  )
  assert.match(
    page,
    /\{PRESET_DISCLAIMER\}/,
    '左栏必须把明示句渲染出来（不许只写在常量里）',
  )
})

test('表是**静态数据**：这一层零 import（所以它不可能联网拉取 / 自动更新）', () => {
  // ⚠ 这条能被观察到**两个方向**：现在 0 处 import ⇒ 绿；谁加一句 `import` ⇒ 红。
  //    （不是"断某串不在场"那种恒真断言 —— 它判的是"这张表有没有长出对外依赖"。）
  const source = read('../src/api/presets.ts')
  const imports = [...source.matchAll(/^\s*import\s/gm)]
  assert.equal(
    imports.length,
    0,
    `预设表随发版走、不做目录服务 ⇒ 这一层不该有任何 import，实际 ${imports.length} 处`,
  )
})

test('★ 左栏一行 = 一家：清单按**地址相等**归并（2026-09-23 二次回炉的核心判据）', () => {
  const deepseek = PROVIDER_PRESETS[0]
  assert.ok(deepseek, '预设表非空')
  const entries = [
    // 同一家的两条（罕见边角：同一家两个 key）⇒ 必须落进**同一个桶**
    { id: 'a', endpoint: deepseek.endpointTemplate },
    { id: 'b', endpoint: deepseek.endpointTemplate },
    // 地址认不出是哪家 ⇒ 归到「自定义」，**不许把它藏起来**
    { id: 'c', endpoint: 'https://my-own-gateway.example/v1/chat/completions' },
  ]
  const grouped = groupEntriesByProvider(entries)
  assert.equal(grouped.get(deepseek.id)?.length, 2, '同一家的两条要归到一起')
  assert.deepEqual(
    grouped.get(deepseek.id)?.map((entry) => entry.id),
    ['a', 'b'],
    '同一家内部**保持传入顺序**（先存的在前）',
  )
  assert.deepEqual(
    grouped.get(CUSTOM_PRESET_ID)?.map((entry) => entry.id),
    ['c'],
    '认不出的地址归「自定义」——它也是真条目该待的地方',
  )
  assert.equal(grouped.size, 2, '两家 ⇒ 两个桶')
})

test('归并**不改身份**：每条只出现在一个桶里，条数守恒', () => {
  const entries = [
    { id: '1', endpoint: PROVIDER_PRESETS[0]!.endpointTemplate },
    { id: '2', endpoint: PROVIDER_PRESETS[1]!.endpointTemplate },
    { id: '3', endpoint: '' },
    { id: '4', endpoint: PROVIDER_PRESETS[0]!.endpointTemplate },
  ]
  const grouped = groupEntriesByProvider(entries)
  const flattened = [...grouped.values()].flat()
  assert.equal(flattened.length, entries.length, '总条数守恒（不吞不重复）')
  assert.equal(
    new Set(flattened.map((entry) => entry.id)).size,
    entries.length,
    '每条只出现在一个桶里',
  )
})

test('左栏状态三档：有住户在用 / 已保存 / 还没配（判据是这一家的条目里**有没有人在用**）', () => {
  assert.equal(providerRowState([], ['x']), 'none', '这一家没条目 ⇒ 还没配')
  assert.equal(providerRowState(['a'], []), 'saved', '有条目但没人在用 ⇒ 已保存')
  assert.equal(providerRowState(['a'], ['b']), 'saved', '在用的那条不在这一家 ⇒ 已保存')
  assert.equal(providerRowState(['a', 'b'], ['b']), 'bound', '在用的那条在这一家里 ⇒ 有住户在用')
  // ★ 2026-10-12（逐支口径）：**在用的是一组** —— 第一支用的不是这条，第二支用着它，仍算"在用"。
  //   从前传单个 id 的那版在这里会说「已保存」（而这一家明明有人用着）。
  assert.equal(providerRowState(['a'], ['c', 'a']), 'bound', '多支里只要有一支用它 ⇒ 在用')
})

// ---- 标记（ProviderMark）：**真 logo**，来自 LobeHub lobe-icons ----
//      （来源 / 许可 / slug 对照 → `src/assets/provider-logos/NOTICE.md`）
//
// 这一层的危险有三个方向：
//
//  1. **漏一家** ⇒ 那一行静默退成首字母，看着像没做完；
//  2. **两个表漂移**（预设加了家、logo 没跟上）—— 最容易出的错；
//  3. **红线**：运行时不发请求（不许那种 `<img src="https://…">` 的写法）。
//     这条是**安全边界**，不是版权 —— 跑起来去拉别人的图，等于页面上写着
//     "这台机器在准备连 DeepSeek"。
//
// ⚠ **历史（别再犯）**：上一版走的是"自绘"，理由是我报的「真 logo 只有 6/18」。
//    那个读数**残缺** —— 只查了官方仓库 + Simple Icons（通用库），**漏了 AI 专用的社区库**。
//    真实读数（逐条 curl）：**单色 18/18、彩色 15/18**。
//    ⇒ **没穷举来源，覆盖率就是假读数。**

const markSource = () => read('../src/components/celira/ProviderMark.tsx')

test('★ 标记表与预设表**一一对应**，且每一份 logo 文件真的在仓库里', () => {
  const source = markSource()
  const block = source.slice(
    source.indexOf('const LOGO_SVG'),
    source.indexOf('export interface', source.indexOf('const LOGO_SVG')),
  )
  const markedIds = [...block.matchAll(/^\s{2}([a-z0-9]+):/gm)].map((m) => m[1]!)
  assert.ok(markedIds.length > 0, '一个标记都没解析出来（LOGO_SVG 的形状变了？）')

  for (const preset of PROVIDER_PRESETS) {
    assert.ok(
      markedIds.includes(preset.id),
      `预设「${preset.id}」没有标记 ⇒ 那一行会静默退成首字母（每家都必须有）`,
    )
    // 光有个键不算数 —— 那份文件要真的在仓库里，而且真是一份 SVG。
    const file = read(`../src/assets/provider-logos/${preset.id}.svg`)
    assert.match(file, /^\s*<svg[\s>]/, `${preset.id}.svg 不是一份 SVG`)
    assert.ok(file.includes('<title>'), `${preset.id}.svg 不像是上游导出的（没有 <title>）`)
  }
  for (const id of markedIds) {
    assert.ok(
      PROVIDER_PRESETS.some((preset) => preset.id === id),
      `标记表里的「${id}」在预设表里不存在（预设删了家、标记没跟上）`,
    )
  }
})

test('★ 标记**零网络**：引的全是本仓文件，没有一处对外地址', () => {
  const source = markSource()
  for (const forbidden of ['fetch(', 'XMLHttpRequest', "from '@/api", 'axios']) {
    assert.ok(
      !source.includes(forbidden),
      `标记组件里出现了「${forbidden}」——标记必须是本仓矢量，零网络`,
    )
  }
  assert.ok(
    !/https?:\/\//.test(source),
    '标记组件里出现了对外地址 —— 只能是本仓文件（红线：运行时不拉别人的图）',
  )
  // 每一份都必须 **`?raw` 从本仓 assets 读进来**（构建期进 bundle ⇒ 运行时不发请求）。
  const imports = [...source.matchAll(/from '([^']*provider-logos[^']*)'/g)].map((m) => m[1]!)
  assert.equal(
    imports.length,
    PROVIDER_PRESETS.length,
    `logo 的 import 数应与预设家数一致，实际 ${imports.length}`,
  )
  for (const path of imports) {
    assert.match(
      path,
      /^@\/assets\/provider-logos\/[a-z0-9-]+\.svg\?raw$/,
      `logo 必须用 ?raw 从本仓读（换成别的方式就可能变成运行时请求）：${path}`,
    )
  }
})

test('★ 认不出 id ⇒ 回落到**首字母**，不编一个形状（编的形状会被当成真标记）', () => {
  const source = markSource()
  assert.ok(/if \(!svg\)/.test(source), '必须有"这家没有图"的那条分支')
  assert.ok(
    source.includes('{name.slice(0, 1)}'),
    '不认识的 id 必须走首字母那条路',
  )
})

test('★ 合规可查：每份 logo 都在 NOTICE.md 里记了来源（不留无出处的图）', () => {
  const notice = read('../src/assets/provider-logos/NOTICE.md')
  assert.match(notice, /lobehub\/lobe-icons/, '必须写明上游仓库')
  assert.match(notice, /MIT/, '必须写明许可')
  for (const preset of PROVIDER_PRESETS) {
    assert.ok(
      notice.includes(`\`${preset.id}.svg\``),
      `NOTICE.md 里没记「${preset.id}.svg」的来源 —— 商标图不许无出处`,
    )
  }
})

test('★ 变体不许漂移：NOTICE.md 说某家是彩色还是单色，文件里就得真是那一版', () => {
  // 现实里会犯的错：重抓时拿了 `<slug>.svg` 而漏了 `<slug>-color.svg`（或反过来）。
  // 界面上只是"这家怎么变成黑白的了"，**没人会去查表** ⇒ 让表与文件互相钉住。
  const notice = read('../src/assets/provider-logos/NOTICE.md')
  const rows = notice
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => /^\|\s*`[a-z0-9-]+\.svg`\s*\|/.test(line))
    .map((line) => line.split('|').map((cell) => cell.trim()))

  assert.ok(rows.length > 0, 'NOTICE.md 的对照表一行都没解析出来（表格式变了？）')
  assert.equal(
    rows.length,
    PROVIDER_PRESETS.length,
    `NOTICE.md 记了 ${rows.length} 家，预设表有 ${PROVIDER_PRESETS.length} 家`,
  )

  for (const [, fileCell, slugCell, variantCell] of rows) {
    const id = fileCell!.replace(/`|\.svg/g, '')
    const slug = slugCell!.replace(/`/g, '')
    const content = read(`../src/assets/provider-logos/${id}.svg`)
    const saysColored = variantCell!.includes('彩色')
    const saysMono = variantCell!.includes('单色')

    assert.ok(
      saysColored !== saysMono,
      `${id} 的「变体」那格既不是彩色也不是单色：「${variantCell}」`,
    )
    // 表自己内部先要对得上：`-color` 结尾 ⇔ 彩色
    assert.equal(
      slug.endsWith('-color'),
      saysColored,
      `${id} 的 slug 与「变体」列自相矛盾：「${slug}」`,
    )
    // 再看**文件真实内容**（判据落在这一层才有用）
    const baked = /(fill|stop-color)="#[0-9a-fA-F]{3,8}"/.test(content)
    if (saysColored) {
      assert.ok(
        baked,
        `${id} 表里标的是**彩色**，但文件里一个烘焙色都没有 ⇒ 重抓时多半漏了 -color`,
      )
      assert.ok(
        !content.includes('fill="currentColor"'),
        `${id} 是彩色版，不该再用 currentColor（那是单色版的写法）`,
      )
    } else {
      assert.ok(
        content.includes('fill="currentColor"'),
        `${id} 表里标的是**单色**，但文件不是 currentColor 版 ⇒ 重抓时多半拿了彩色版`,
      )
      assert.ok(
        !baked,
        `${id} 是单色版，不该有烘焙的颜色（单色要跟着主题明暗走）`,
      )
    }
  }
})

// ---------------------------------------------------------------------------
// 「去哪儿拿钥匙」（2026-09-24）：一张纯外部链接表
// ---------------------------------------------------------------------------

test('★ 每条预设都有「去哪儿拿钥匙」的链接（这是用户最容易卡住的一步）', () => {
  for (const preset of PROVIDER_PRESETS) {
    const links = providerLinks(preset.id)
    assert.ok(links, `${preset.id} 没有链接 —— 用户知道要贴 key，但不知道去哪儿申请`)
    // ⚠ 只钉**形状**，不钉具体 URL：钉死地址就等于「守卫写死被守卫对象的文案」——
    //    厂商改版时判据会红得莫名其妙，而真正该管的是"它得像个真链接、不是占位符"。
    for (const [field, url] of Object.entries(links!)) {
      assert.match(url, /^https:\/\/[^ ]+\.[^ ]+/, `${preset.id}.${field} 不像 https 链接：${url}`)
      assert.ok(!/todo|placeholder|example\.com|xxx/i.test(url), `${preset.id}.${field} 是占位符：${url}`)
    }
  }
})

test('「自定义」不是预设 ⇒ 没有链接（`null`：不回落、不猜）', () => {
  assert.equal(providerLinks(CUSTOM_PRESET_ID), null)
  assert.equal(providerLinks(null), null)
  assert.equal(providerLinks(''), null)
  assert.equal(providerLinks('不存在的家'), null)
})

// ---------------------------------------------------------------------------
// 「试连会打向哪」：页面那份与门那份**必须同源**（2026-09-24）
// ---------------------------------------------------------------------------

const { modelsUrl: doorModelsUrl } = createRequire(import.meta.url)('../../scripts/home/config-server.js') as {
  modelsUrl: (endpoint: string) => string
}

test('★★ 两侧同源：页面算的「试连会打向哪」与门那份**逐项一致**（跨运行时的口径不许漂）', () => {
  // 门那边是**权威**（真发请求的是它）；页面这份只是为了让用户先看见。
  // ⇒ 这条判据就是防"两份实现各写各的、改了这边忘了那边"。
  const cases = [
    'https://api.deepseek.com/v1/chat/completions',
    'https://api.openai.com/v1/responses',
    'https://api.siliconflow.cn/v1/chat/completions',
    'https://ark.cn-beijing.volces.com/api/v3/chat/completions',
    'https://api.deepseek.com/v1/completions', // 老式 completions 也算
    'https://api.deepseek.com/v1', // 只到基址 ⇒ 原样拼
    'https://api.deepseek.com', // 只到域名 ⇒ 原样拼（**后果由页面那行小字暴露**）
    'https://x.com/v1/chat/completions/', // 末尾多一个斜杠
    // ⚠ **空串不参与比对**：门的 `endpoint` 由校验保证恒非空（`validateDeclaration` 挡在前面），
    //    而页面那份在空串时返回 `''` —— 那是**给 UI 的哨兵**（"没地址就别显示那行小字"），
    //    不是"另一种口径"。两者的差别只落在"门永远不会收到的输入"上 ⇒ 不算漂。
  ]
  for (const endpoint of cases) {
    assert.equal(
      probeUrlFor(endpoint),
      doorModelsUrl(endpoint),
      `口径漂了：${endpoint || '(空)'} ⇒ 页面 ${probeUrlFor(endpoint)} ≠ 门 ${doorModelsUrl(endpoint)}`,
    )
  }
})

test('★ 每条预设的地址都算得出一个像 `<基址>/models` 的试连地址（否则那行小字会误导人）', () => {
  for (const preset of PROVIDER_PRESETS) {
    const probe = probeUrlFor(preset.endpointTemplate)
    assert.match(probe, /^https:\/\/[^ ]+\/models$/, `${preset.id} 的试连地址不像话：${probe}`)
    assert.ok(!probe.includes('/models/models'), `${preset.id} 摘尾没摘干净：${probe}`)
  }
})
