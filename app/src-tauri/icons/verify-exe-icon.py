"""验收读数：**图标到底占多少**（"看起来这么小"那一刀）。

两件事分开做，因为它们回答不同的问题：
① **占多少** —— 量 `icon.ico` 里各档的可见占比（Pillow 读得到 alpha ✓）。
   ⚠ 不能拿 `ExtractAssociatedIcon` 去量：GDI+ 交出来的是**不透明位图**，
   alpha 全丢 ⇒ 量出来恒是 100%（踩过）。
② **是不是那个标记** —— 从编好的 exe 里抠出来看一眼（形状对了就够，不量尺寸）。
"""

import os
import subprocess

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
# exe 从**本文件的位置**折出来，不写死本机路径（`HERE` 是 app/src-tauri/icons）。
EXE = os.path.normpath(os.path.join(HERE, "..", "target", "release", "celira-app.exe"))
OUT = os.path.join(HERE, "_exe-icon-check.png")

os.chdir(HERE)

print("① icon.ico 各档的可见占比（判据：占高 ~94%）")
ico = Image.open("icon.ico")
for size in sorted(ico.info.get("sizes", [])):
    ico.size = size
    ico.load()
    frame = ico.convert("RGBA")
    box = frame.getbbox()
    if box is None:
        print(f"  {size[0]}px：整档透明 ✗")
        continue
    print(
        f"  {size[0]}px：占宽 {(box[2] - box[0]) / size[0]:.1%}、占高 {(box[3] - box[1]) / size[1]:.1%}"
    )

print("② 从 exe 里抠出来的那一枚（只看形状）")
ps = f"""
Add-Type -AssemblyName System.Drawing
$ico = [System.Drawing.Icon]::ExtractAssociatedIcon('{EXE}')
$bmp = $ico.ToBitmap()
$bmp.Save('{OUT}', [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose(); $ico.Dispose()
"""
subprocess.run(["powershell", "-NoProfile", "-Command", ps], check=True)
print(f"  已写到 {OUT}（{Image.open(OUT).size[0]}px）")
