"""从 ICON.webp 生成 Tauri 需要的图标集。

注意：ICON.webp 自带圆角深蓝底（964x964）。这里只生成"传统图标"（桌面 + Android legacy），
Android 自适应图标需要"透明前景 + 纯色背景"两层，留到 M4 单独裁切，否则系统遮罩会让圆角叠成双重圆角。

用法： python scripts/gen_icons.py
"""

from __future__ import annotations

import shutil
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "ICON.webp"
ICONS_DIR = ROOT / "src-tauri" / "icons"
ASSETS_DIR = ROOT / "src" / "assets"

PNG_SIZES = {
    "32x32.png": 32,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 1024,
}

ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]


def main() -> None:
    if not SRC.exists():
        raise SystemExit(f"找不到源图：{SRC}")

    ICONS_DIR.mkdir(parents=True, exist_ok=True)
    ASSETS_DIR.mkdir(parents=True, exist_ok=True)

    image = Image.open(SRC).convert("RGBA")
    print(f"source: {SRC.name} {image.width}x{image.height}")

    for name, size in PNG_SIZES.items():
        image.resize((size, size), Image.LANCZOS).save(ICONS_DIR / name, "PNG")

    ico_base = image.resize((256, 256), Image.LANCZOS)
    ico_base.save(ICONS_DIR / "icon.ico", sizes=ICO_SIZES)

    # 应用内 Logo：Vite 直接消费 webp，无需转换
    shutil.copyfile(SRC, ASSETS_DIR / "logo.webp")

    for path in sorted(ICONS_DIR.iterdir()):
        print(f"  {path.name:>16}  {path.stat().st_size / 1024:6.1f} KB")
    print("done")


if __name__ == "__main__":
    main()
