// 文档诚实声明守卫 —— `agent/README.md` 的 §3.x 改写不许删掉"如实登记"。
//
// 填哪个洞（2026-10-17 立）
// ---------------------------------------------------------------------------
// §3.x 正在被逐节改写：把"这一刀的经过"从"当前事实"里剥掉。这件事的判据是
// 「**能压的是过程，压不动的是事实**」——而过程与事实在正文里**长得一模一样**，
// 唯一的机器可辨差别是：**事实里有一部分是"承认自己不知道"的句子**
// （未覆盖 / 未裁 / 如实登记 / 触发条件 / 不假装 / 今天不可达 …）。
//
// 那些句子**字节很少、价值最高**：它们是这份文档唯一敢说"这里我没做到"的地方，
// 而删掉它们**不会让任何测试变红、不会让任何引用失效、读起来还更顺**。
// ⇒ 一次"压缩"里最容易被顺手删掉的，恰好是最不该删的东西。这个守卫就是为它写的。
//
// 判据方向（写死，不改）
// ---------------------------------------------------------------------------
// **拿"已提交的版本"当基线，拿"工作树"当被测**：
//
//   硬判据（红）：某节在基线里出现过的**标记种类**，在工作树里**一个都不许少**。
//                 少一种 ⇒ 那一类诚实声明被整类删掉了。
//   软读数（只报）：同一节的**标记条数**减少 ⇒ 打印出来给人看，**不判红**。
//                 （合并两句同义声明是合法的；只有"整类消失"才一定是删事实。）
//
// 为什么用 HEAD 当基线而不是快照文件：**"改 §3.x 之前先跑它"** 是它的使用姿势——
// 基线永远是"上一次提交的样子"，不需要人维护第二份副本。
// ⇒ **它的触发条件**：改了 `agent/README.md` 的 §3.x 正文 ⇒ 跑它（`npm run check:docs` 一起跑）。
//
// 量得到 / 量不到（如实登记，不假装）
// ---------------------------------------------------------------------------
// 量得到：**整类诚实声明被删**（硬判据）；**标记条数下降**（软读数）。
// 量不到：① **不带标记的事实被删**（例如"某个上限是 600 秒"这类，正文里没有标记词）
//         ——这一格**只有人核**，守卫一个字都帮不上；
//         ② **改写把话说反了**（标记还在、意思反了）；
//         ③ **新增的节**（基线里没有的节不查——没有可比对象）。
// ⚠ 所以它**不是"改写合格"的证明**，只是"没把承认无知的句子删光"的下界。
//
// 退出码：0 = 通过 · 1 = 有节少了一整类标记
//
// 用法：node scripts/check-doc-honesty.mjs

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..');
const DOC = 'agent/README.md';

/** 诚实声明的封闭词表。**加词要写理由**——每个词都是"这份文档承认自己不知道"的一种说法。 */
const MARKERS = [
  ['未覆盖', '明确列出没做到的门禁/证据缺口'],
  ['未裁', '登记了但等主人裁决'],
  ['待裁', '同上，另一种写法'],
  ['如实登记', '把取舍/偏离摆在明处'],
  ['登记在案', '同上，另一种写法'],
  ['不假装', '拒绝把没做到的写成做到了'],
  ['触发条件', '写明"什么时候该回来动它"'],
  ['今天不可达', '登记了形状债但不假装它会发作'],
  ['开放问题', '明确标为未决'],
  ['缺什么', '负空间：这一格明确没有什么'],
];

/**
 * 从全文里切出 §3.x 各节的正文（`### N.NN …` 到下一个 `##`/`###` 之前）。
 *
 * ⚠ **键归一掉字母后缀、并把同一族的正文并起来**（`3.93` 与 `3.93a` 都算 `3.93`）。
 * 两个理由，都是实测踩出来的：
 *   ① **历史上有过重复节号**（`3.93` / `3.94` / `3.95` 各出现过两轮），按号做键会让
 *      **后一节静默覆盖前一节** ⇒ 拿两个不同的节互比，报出假阳性；
 *   ② 重复号后来按仓里的先例加了后缀（`3.93a`）**修复**，而基线里没有后缀 ⇒
 *      不归一就会把"同一节改了名"读成"整节消失"。
 * ⇒ 归一后的判据是「**这一族（同号及其后缀）的诚实声明不许整类变少**」，对改名免疫。
 */
function sections(text) {
  const lines = text.split('\n');
  const heads = [];
  lines.forEach((line, index) => {
    const m = /^#{2,3} (\d+(?:\.\d+)*[a-z]?)\.?\s/.exec(line);
    if (m) heads.push({ index, num: m[1] });
  });
  const out = new Map();
  heads.forEach((head, i) => {
    if (!head.num.startsWith('3.')) return;
    const key = head.num.replace(/[a-z]$/, '');
    const end = i + 1 < heads.length ? heads[i + 1].index - 1 : lines.length - 1;
    const body = lines.slice(head.index, end + 1).join('\n');
    out.set(key, out.has(key) ? `${out.get(key)}\n${body}` : body);
  });
  return out;
}

/** 一节里每种标记出现几次。 */
function tally(body) {
  const counts = new Map();
  for (const [word] of MARKERS) {
    const hits = body.split(word).length - 1;
    if (hits > 0) counts.set(word, hits);
  }
  return counts;
}

/** 比较基线与被测，返回 {missing, shrank}。 */
export function compare(baselineText, currentText) {
  const base = sections(baselineText);
  const cur = sections(currentText);
  const missing = [];
  const shrank = [];
  for (const [num, body] of base) {
    const now = cur.get(num);
    if (now === undefined) {
      missing.push({ num, word: '(整节)', was: 1, is: 0 });
      continue;
    }
    const was = tally(body);
    const is = tally(now);
    for (const [word, count] of was) {
      const got = is.get(word) ?? 0;
      if (got === 0) missing.push({ num, word, was: count, is: 0 });
      else if (got < count) shrank.push({ num, word, was: count, is: got });
    }
  }
  return { missing, shrank, baseSections: base.size, curSections: cur.size };
}

function readBaseline() {
  return execFileSync('git', ['show', `HEAD:${DOC}`], { cwd: repoRoot, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
}

/** 植入违规：把某一节里某个标记整类删掉 ⇒ 必须被抓住。 */
function selfCheck() {
  const sample = [
    '### 3.1 甲',
    '',
    '这一格**未覆盖**：没有行为证明。',
    '',
    '### 3.2 乙',
    '',
    '这一格如实登记：不做。触发条件：第二个消费者出现时。',
    '',
  ].join('\n');
  const broken = sample.replace('**未覆盖**', '**已覆盖**');
  const clean = compare(sample, sample);
  const caught = compare(sample, broken);
  const ok =
    clean.missing.length === 0 &&
    caught.missing.length === 1 &&
    caught.missing[0].num === '3.1' &&
    caught.missing[0].word === '未覆盖';
  if (!ok) {
    console.error('[check-doc-honesty] selfCheck 失败：植入的违规没被抓到 —— 否则这个守卫就是装饰');
    console.error(JSON.stringify({ clean, caught }, null, 2));
    process.exit(1);
  }
  return { plantedViolations: 1 };
}

const baseline = readBaseline();
const current = readFileSync(join(repoRoot, DOC), 'utf8');
const self = selfCheck();
const { missing, shrank, baseSections, curSections } = compare(baseline, current);

if (shrank.length > 0) {
  console.log(`[check-doc-honesty] 软读数（不判红）：${shrank.length} 处标记条数下降 —— 合并同义声明是合法的，请人核一眼：`);
  for (const s of shrank.slice(0, 20)) console.log(`    §${s.num}  「${s.word}」${s.was} → ${s.is}`);
  if (shrank.length > 20) console.log(`    …另有 ${shrank.length - 20} 处`);
}

if (missing.length > 0) {
  console.error(`[check-doc-honesty] 失败：${missing.length} 处诚实声明被整类删掉`);
  for (const m of missing) console.error(`    §${m.num}  「${m.word}」${m.was} → 0`);
  console.error('  ⇒ 若这是有意为之，请把理由写进那一节；不许静默删掉"承认自己不知道"的句子。');
  process.exit(1);
}

console.log(
  `[check-doc-honesty] 通过：§3.x ${curSections} 节（基线 ${baseSections} 节）· ` +
    `封闭词表 ${MARKERS.length} 个 · 自检植入 ${self.plantedViolations} 条违规已抓住 · ` +
    `软读数 ${shrank.length} 处下降`,
);
