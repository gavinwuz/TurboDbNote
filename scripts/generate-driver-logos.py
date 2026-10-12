"""Render checked-in brand sources; maintenance only: resvg-py and Pillow."""
from io import BytesIO
from pathlib import Path
import hashlib
import json
import re
from PIL import Image
import resvg_py

ROOT = Path(__file__).resolve().parents[1] / "crates/desktop/assets/brands"


def main():
    manifest = json.loads((ROOT / "sources.json").read_text())
    for driver, item in manifest.items():
        source = ROOT / item["file"]
        data = source.read_bytes()
        if hashlib.sha256(data).hexdigest() != item["sha256"]:
            raise ValueError("Source checksum mismatch: " + driver)
        if source.suffix == ".svg":
            svg = data.decode("utf-8")
            # Some official Illustrator exports contain an empty editor-only fallback.
            svg = re.sub(r"<foreignObject\b[^>]*>\s*</foreignObject>", "", svg, flags=re.I)
            if re.search(r"<(?:script|foreignObject)\b|(?:href\s*=\s*[\"'](?:https?:|file:))", svg, re.I):
                raise ValueError("External/active SVG content: " + driver)
            if item.get("tint"):
                svg = svg.replace("<svg ", '<svg fill="' + item["tint"] + '" ', 1)
            data = resvg_py.svg_to_bytes(svg_string=svg, width=256)
        image = Image.open(BytesIO(data)).convert("RGBA")
        bounds = image.getbbox()
        if not bounds:
            raise ValueError("Empty artwork: " + driver)
        image = image.crop(bounds)
        image.thumbnail((112, 112), Image.Resampling.LANCZOS)
        canvas = Image.new("RGBA", (128, 128))
        canvas.alpha_composite(image, ((128 - image.width) // 2, (128 - image.height) // 2))
        canvas.save(ROOT / (driver + ".png"), optimize=True)
    print("Rendered", len(manifest), "embedded driver logos")


if __name__ == "__main__":
    main()
