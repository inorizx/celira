"""生成应用图标那一整套（Windows 的 .ico、各档 PNG、磁贴、以及 macOS 的 .icns）。

## 为什么有这一支

`icons/icon.png` 从前是 **Tauri 的默认图标**（蓝黄那枚）—— 产品自己的标记是
`client/public/logo/logo-mark.svg`（`--brand: #a284f4`）。2026-10-17 主人问"我们没有自己的
celira logo 吗，作为应用"，就是这件事：窗口、任务栏、安装包、磁贴都还是默认那枚。

## 两步

1. `cd client && node scripts/render-logo.mjs` ⇒ `icons/source-mark.png`（1024×1024 透明底）；
2. `python icons/make-icons.py` ⇒ 本脚本写出全套（**产物进仓库**）。

脚本留在仓库里是为了**可复核**：谁都能重跑一遍得到同一套图（与 `make-tray-running.py` 同规）。

## 形状判据

- **透明底**：深浅任务栏、磁贴、浅色标题栏都要能用（不许带白底）；
- **按真实像素边界裁掉空白**（2026-10-17 主人当场："为啥看起来这么小呢"）：标记在
  `logo-mark.svg` 的 1024 画布里只画到 `(133,112)-(874,912)` ⇒ **只占 72% 宽、78% 高**，
  再叠一层留白就只剩 ~61% ✗ —— 任务栏里比旁边那些满格的应用小一圈。
  现在先裁到像素边界，再按平台给小边距（Windows 3% ⇒ 可见部分 ~94%，与满格应用齐平）；
- **不许拉伸**：标记是 741×800（略高于宽）⇒ 按**长边**缩放居中，比例不动。
"""

from PIL import Image

SRC = "source-mark.png"
# 裁掉空白之后，再给多少边距（占边长）。Windows 那几档要"满格"；macOS 惯例是留得多一些。
PAD_WINDOWS = 0.03
PAD_MACOS = 0.12
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)

# Windows 磁贴 / 商店图标（Tauri 模板里那几张，尺寸照抄，别改）
TILES = {
    "Square30x30Logo.png": 30,
    "Square44x44Logo.png": 44,
    "Square71x71Logo.png": 71,
    "Square89x89Logo.png": 89,
    "Square107x107Logo.png": 107,
    "Square142x142Logo.png": 142,
    "Square150x150Logo.png": 150,
    "Square284x284Logo.png": 284,
    "Square310x310Logo.png": 310,
    "StoreLogo.png": 50,
}

# .icns 里嵌 PNG 的档位（类型码 → 像素边长）
ICNS = {
    b"ic07": 128,
    b"ic08": 256,
    b"ic09": 512,
    b"ic10": 1024,
    b"ic11": 32,
    b"ic12": 64,
    b"ic13": 256,
    b"ic14": 512,
}


def _art(src: Image.Image) -> Image.Image:
    """裁到**真实像素边界**（透明空白不算内容）—— 这是"看起来小"那一刀的判据本体。"""
    box = src.getbbox()
    if box is None:
        raise SystemExit("source-mark.png 整张都是透明的 —— 上游渲染那一步坏了")
    return src.crop(box)


def _square(src: Image.Image, size: int, pad: float = PAD_WINDOWS) -> Image.Image:
    """把标记放进 size×size 的透明画布（**按长边**等比缩放 + 平台边距，绝不拉伸）。"""
    art = _art(src)
    inner = int(size * (1 - pad * 2))
    scale = inner / max(art.size)
    scaled = art.resize(
        (max(1, round(art.width * scale)), max(1, round(art.height * scale))), Image.LANCZOS
    )
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.paste(
        scaled, ((size - scaled.width) // 2, (size - scaled.height) // 2), scaled
    )
    return canvas


def _png_bytes(img: Image.Image) -> bytes:
    import io

    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def write_icns(img: Image.Image) -> None:
    entries = b""
    for kind, size in ICNS.items():
        payload = _png_bytes(_square(img, size, PAD_MACOS))
        entries += kind + (len(payload) + 8).to_bytes(4, "big") + payload
    body = b"icns" + (len(entries) + 8).to_bytes(4, "big") + entries
    with open("icon.icns", "wb") as fh:
        fh.write(body)
    print(f"写出 icon.icns：{len(ICNS)} 档，{len(body)} 字节")


def report_fill(name: str) -> None:
    """写出之后**量一遍**：可见部分占多少（判据：Windows 那几档 ~94%）。"""
    img = Image.open(name).convert("RGBA")
    box = img.getbbox()
    if box is None:
        raise SystemExit(f"{name} 整张透明 —— 生成坏了")
    print(
        f"  {name}: {img.size[0]}px，可见占宽 {(box[2] - box[0]) / img.size[0]:.1%}"
        f"、占高 {(box[3] - box[1]) / img.size[1]:.1%}"
    )


def main() -> None:
    src = Image.open(SRC).convert("RGBA")

    # ① 桌面那几档（`tauri.conf.json` 的 bundle.icon 点名了前三张 + .ico + .icns）
    _square(src, 1024).save("icon.png")
    _square(src, 32).save("32x32.png")
    _square(src, 128).save("128x128.png")
    _square(src, 256).save("128x128@2x.png")
    print("写出 icon.png / 32x32.png / 128x128.png / 128x128@2x.png")

    # ② Windows 的 .ico：多档塞在一个文件里，系统按显示尺寸自己挑
    ico = _square(src, 256)
    ico.save("icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    print("写出 icon.ico（16/24/32/48/64/128/256）")

    # ③ macOS 的 .icns（手写容器：Pillow 只能读不能写）
    write_icns(src)

    # ④ Windows 磁贴 / 商店
    for name, size in TILES.items():
        _square(src, size).save(name)
    print(f"写出磁贴 {len(TILES)} 张")

    # ⑤ **量一遍**（"看起来小"那一刀的判据：可见部分要接近满格，不再是一圈空白）
    print("可见占比（Windows 那几档应 ~94%）：")
    for name in ("icon.png", "32x32.png", "128x128.png", "StoreLogo.png"):
        report_fill(name)

    print("下一步：python make-tray-running.py（托盘那一版从 icon.png 派生）")


if __name__ == "__main__":
    main()
