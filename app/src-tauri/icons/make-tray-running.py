"""生成托盘那个「还有本地服务在跑」的图标变体。

§8.2 的原话：**托盘图标要能看出"还有本地服务在跑"**。否则用户会以为已经全关了，
而东西还在后台写磁盘、占端口。

做法：拿 app 自己的图标（`icons/icon.png`），在右下角画一个**实心绿点**——
两态一眼可辨（有服务在跑 / 都停了），不引入新的视觉语言（绿点就是"活着"，
与 app 里"在线"那一格同一条语义）。

只生成一次；产物进仓库（`icons/tray-running.png`）。这个脚本留在仓库里是为了
**可复核**（谁都能重跑一遍得到同一个形状），不是构建步骤。
"""

from PIL import Image, ImageDraw

SRC = "icon.png"
DST = "tray-running.png"
SIZE = 256  # 托盘图标会被系统缩到 16/20/24 px，画大一点再让系统缩
DOT_RATIO = 0.30  # 绿点直径占边长
MARGIN_RATIO = 0.02


def main() -> None:
    base = Image.open(SRC).convert("RGBA").resize((SIZE, SIZE), Image.LANCZOS)
    draw = ImageDraw.Draw(base)
    dot = int(SIZE * DOT_RATIO)
    margin = int(SIZE * MARGIN_RATIO)
    x0 = SIZE - dot - margin
    y0 = SIZE - dot - margin
    # 绿点带一圈底色描边：深色与浅色任务栏上都看得见（描边取图标自己的暗色系）。
    ring = max(2, dot // 8)
    draw.ellipse(
        [x0 - ring, y0 - ring, x0 + dot + ring, y0 + dot + ring],
        fill=(18, 16, 28, 255),
    )
    draw.ellipse([x0, y0, x0 + dot, y0 + dot], fill=(52, 199, 89, 255))
    base.save(DST)
    print(f"写出 {DST}：{SIZE}x{SIZE}，绿点 {dot}px（右下角）")


if __name__ == "__main__":
    main()
