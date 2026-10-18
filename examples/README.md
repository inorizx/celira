# examples/

这里的每一份都是**能直接抄的样例**——不是文档，是"拿起来就能用"的东西。

| 样例 | 是哪一格 | 怎么用 |
|---|---|---|
| [`skill-pack-receipt-check/`](skill-pack-receipt-check/) | **方法**（技能包） | 把整个目录拷进 `<家>/materials/skills/`，下一段清醒期开场读数就变 |

**这一格为什么先有样例**：四格里只有它**不需要写代码、不需要凭据、不需要重启**——
所以它是最短的第一次贡献路径。四格的全貌与另外三格的入口见仓根 [`EXTENDING.md`](../EXTENDING.md)。

⚠ **样例是形状，不是规范**：技能包的唯一权威是
[`agent/SKILL_PACK_GUIDE.md`](../agent/SKILL_PACK_GUIDE.md)（物理约定）与
`agent/src/skills_desktop.rs`（实现）。本目录里的样例如果和那两处不一致，以那两处为准。
