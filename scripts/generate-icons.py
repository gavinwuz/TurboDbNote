"""Regenerate checked-in Windows icon: pip install resvg-py==0.5.0 Pillow."""
from io import BytesIO
from pathlib import Path
import struct

from PIL import Image, ImageDraw
import resvg_py

ASSETS = Path(__file__).resolve().parents[1] / "crates/desktop/assets"
SIZES = (16, 20, 24, 32, 48, 64, 128, 256)


def dib_frame(png, size):
    """Classic 32-bit DIB + AND mask for legacy Windows icon consumers."""
    image = Image.open(BytesIO(png)).convert("RGBA")
    pixels = image.tobytes("raw", "BGRA", 0, -1)
    stride = ((size + 31) // 32) * 4
    mask = bytearray(stride * size)
    for y in range(size):
        for x in range(size):
            if image.getpixel((x, size - 1 - y))[3] == 0:
                mask[y * stride + x // 8] |= 0x80 >> (x % 8)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2,
                         1, 32, 0, len(pixels) + len(mask), 0, 0, 0, 0)
    return header + pixels + mask


def main():
    # Render every resolution from the SVG, rather than upscaling a small bitmap.
    frames = [resvg_py.svg_to_bytes(svg_path=str(ASSETS / "data-note-app.svg"),
                                   width=size, height=size) for size in SIZES]
    # Small frames use traditional DIB encoding; retain PNG only for 256px.
    resources = [dib_frame(frame, size) if size < 256 else frame
                 for size, frame in zip(SIZES, frames)]
    offset = 6 + 16 * len(resources)
    entries = []
    for size, frame in zip(SIZES, resources):
        entries.append(struct.pack("<BBBBHHII", size % 256, size % 256,
                                   0, 0, 1, 32, len(frame), offset))
        offset += len(frame)
    (ASSETS / "turbodbnote.ico").write_bytes(
        struct.pack("<HHH", 0, 1, len(resources)) + b"".join(entries) + b"".join(resources))

    # Preview at actual pixel sizes on both light and dark backgrounds.
    preview = Image.new("RGB", (640, 400), "white")
    draw = ImageDraw.Draw(preview)
    draw.rectangle((0, 200, 640, 400), fill="#202A4A")
    for top, color in ((0, "#202A4A"), (200, "#FFFFFF")):
        draw.text((20, top + 14), "TurboDbNote / Data Note", fill=color)
        x = 20
        for size, frame in zip(SIZES[:-1], frames[:-1]):
            icon = Image.open(BytesIO(frame)).convert("RGBA")
            preview.paste(icon, (x, top + 45), icon)
            draw.text((x, top + 180), str(size), fill=color)
            x += size + 18
    preview.save(ASSETS / "icon-preview.png")
    with Image.open(ASSETS / "turbodbnote.ico") as icon:
        assert icon.ico.sizes() == {(size, size) for size in SIZES}
    print("Generated ICO with 8 resolutions and icon-preview.png")


if __name__ == "__main__":
    main()
