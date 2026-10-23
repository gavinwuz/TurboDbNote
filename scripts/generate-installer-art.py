"""Regenerate checked-in Inno wizard branding from the existing application SVG.
Optional development tools: the same Pillow/resvg-py used by generate-icons.py.
Normal builds consume the checked-in BMP files and do not run this script.
"""
from io import BytesIO
from pathlib import Path
from PIL import Image
import resvg_py

root = Path(__file__).resolve().parents[1]
source = root / "crates/desktop/assets/data-note-app.svg"
output = root / "installer/assets"
output.mkdir(exist_ok=True)
for scale in (1, 2, 3):
    suffix = "" if scale == 1 else f"@{scale}x"
    for name, size, logo_size, position, background in (
        ("update-wizard", (164, 314), 120, (22, 52), "#202A4A"),
        ("update-small", (55, 55), 48, (3, 3), "#FFFFFF"),
    ):
        image = Image.new("RGB", tuple(v * scale for v in size), background)
        logo = Image.open(BytesIO(resvg_py.svg_to_bytes(
            svg_path=str(source), width=logo_size * scale, height=logo_size * scale))).convert("RGBA")
        image.paste(logo, tuple(v * scale for v in position), logo)
        image.save(output / f"{name}{suffix}.bmp")
        assert image.size == tuple(v * scale for v in size)
print("Generated wizard branding at 100%, 200% and 300% from data-note-app.svg")
