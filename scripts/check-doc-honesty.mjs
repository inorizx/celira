// 文档诚实声明守卫 —— `agent/README.md` 的改写不许删掉"如实登记"。
//
// 填哪个洞（2026-10-17 立；2026-10-19 改成**全文棘轮**）
// ---------------------------------------------------------------------------
// 这份文档正在被**按主题重组**：把按落地顺序记的节归并进按主题记的节。这件事的判据是
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
//   硬判据（红）：基线里出现过的**标记种类**，在工作树里**一个都不许少**（按**全文**数）。
//                 少一种 ⇒ 那一类诚实声明被整类删掉了。
//   软读数（只报）：同一标记的**总条数**减少 ⇒ 打印出来给人看，**不判红**。
//                 （合并两句同义声明是合法的；只有"整类消失"才一定是删事实。）
//
// ⚠ **为什么从"逐节"改成"全文"**（2026-10-19，这一条是设计，别改回去）：
// 逐节比对**必然挡住重组**——归并两个节、删一个并入的壳节，在逐节视角下都长得像
// "整节消失"或"某一类标记变少"。而重组正是这份文档唯一能真正变短的路径
// （实测：逐节压缩一节 115 行只省 12 行）。⇒ 结构可以动，**诚实声明一条都不许丢**。
// 同一条判据 `check-doc-current.mjs` 早就写着（"为什么是总数棘轮，不是逐节棘轮"）。
//
// 为什么用 HEAD 当基线而不是快照文件：**"改这份文档之前先跑它"** 是它的使用姿势——
// 基线永远是"上一次提交的样子"，不需要人维护第二份副本。
// ⇒ **它的触发条件**：改了 `agent/README.md` 正文 ⇒ 跑它（`npm run check:docs` 一起跑）。
//
// 量得到 / 量不到（如实登记，不假装）
// ---------------------------------------------------------------------------
// 量得到：**整类诚实声明被删**（硬判据）；**标记总条数下降**（软读数）。
// 量不到：① **不带标记的事实被删**（例如"某个上限是 600 秒"这类，正文里没有标记词）
//         ——这一格**只有人核**，守卫一个字都帮不上；
//         ② **改写把话说反了**（标记还在、意思反了）；
//         ③ **搬到别的文件去了**（那可能是对的搬家，也可能是静默删除——只有人核）。
// ⚠ 所以它**不是"改写合格"的证明**，只是"没把承认无知的句子删光"的下界。
//
// 退出码：0 = 通过 · 1 = 有整类标记消失
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

/** 全份文档里每种标记出现几次。 */
function tally(text) {
  const counts = new Map();
  for (const [word] of MARKERS) {
    const hits = text.split(word).length - 1;
    if (hits > 0) counts.set(word, hits);
  }
  return counts;
}

/**
 * 比较基线与被测，返回 `{missing, shrank}`。
 *
 * **按全文数，不按节数**（见文件头「为什么从逐节改成全文」）：结构（分节、标题、顺序）
 * 可以任意动，只要**每一种曾在基线里出现过的诚实标记，在工作树里至少还剩一处**。
 * 导出签名 `compare(baselineText, currentText)` 保持字典调用方不变。
 */
export function compare(baselineText, currentText) {
  const was = tally(baselineText);
  const is = tally(currentText);
  const missing = [];
  const shrank = [];
  for (const [word, count] of was) {
    const got = is.get(word) ?? 0;
    if (got === 0) missing.push({ word, was: count, is: 0 });
    else if (got < count) shrank.push({ word, was: count, is: got });
  }
  return { missing, shrank };
}

function readBaseline() {
  return execFileSync('git', ['show', `HEAD:${DOC}`], { cwd: repoRoot, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
}

/** 植入违规：把某类标记整类删掉 ⇒ 必须被抓住。 */
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
    caught.missing[0].word === '未覆盖' &&
    caught.missing[0].is === 0;
  if (!ok) {
    console.error('[check-doc-honesty] selfCheck 失败：植入的违规没被抓到 —— 否则这个守卫就是装饰');
    console.error(JSON.stringify({ clean, caught }, null, 2));
    process.exit(1);
  }
  // 第二条自检：**重组必须被放行** —— 两节并成一节、标题全改，标记一个不少 ⇒ 不许红。
  const regrouped = [
    '## 3. 甲与乙（并成一节）',
    '',
    '这一格**未覆盖**：没有行为证明。',
    '',
    '这一格如实登记：不做。触发条件：第二个消费者出现时。',
    '',
  ].join('\n');
  const merged = compare(sample, regrouped);
  if (merged.missing.length !== 0) {
    console.error('[check-doc-honesty] selfCheck 失败：合法的重组被拦住了（结构不是判据）');
    console.error(JSON.stringify(merged, null, 2));
    process.exit(1);
  }
  return { plantedViolations: 1, regroupAllowed: true };
}

const baseline = readBaseline();
const current = readFileSync(join(repoRoot, DOC), 'utf8');
const self = selfCheck();
const { missing, shrank } = compare(baseline, current);

if (shrank.length > 0) {
  console.log(`[check-doc-honesty] 软读数（不判红）：${shrank.length} 类标记条数下降 —— 合并同义声明是合法的，请人核一眼：`);
  for (const s of shrank) console.log(`    「${s.word}」${s.was} → ${s.is}`);
}

if (missing.length > 0) {
  console.error(`[check-doc-honesty] 失败：${missing.length} 类诚实声明被整类删掉`);
  for (const m of missing) console.error(`    「${m.word}」${m.was} → 0`);
  console.error('  ⇒ 若这是有意为之，请把理由写进文档；不许静默删掉"承认自己不知道"的句子。');
  process.exit(1);
}

const left = [...tally(current).values()].reduce((a, b) => a + b, 0);
const was = [...tally(baseline).values()].reduce((a, b) => a + b, 0);
console.log(
  `[check-doc-honesty] 通过：诚实标记 ${left} 条（基线 ${was} 条）· ` +
    `封闭词表 ${MARKERS.length} 个 · 自检：植入 ${self.plantedViolations} 条违规已抓住、` +
    `合法重组已放行 · 软读数 ${shrank.length} 类下降`,
);
