"""Generate Pulse app icons (png + ico) without external assets."""
from PIL import Image, ImageDraw
import math, os

OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")
OUT = os.path.abspath(OUT)
os.makedirs(OUT, exist_ok=True)

BG = (24, 26, 31, 255)
LINE = (74, 222, 168, 255)
LINE_ALT = (46, 160, 118, 255)


def rounded_mask(size, radius):
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return m


def pulse_points(w, h, amp):
    """ECG-like waveform across the tile, normalised coordinates."""
    pts = [
        (0.05, 0.50),
        (0.20, 0.50),
        (0.26, 0.38),
        (0.32, 0.62),
        (0.38, 0.16),
        (0.44, 0.84),
        (0.50, 0.44),
        (0.56, 0.50),
        (0.72, 0.50),
        (0.78, 0.42),
        (0.84, 0.58),
        (0.90, 0.50),
        (0.95, 0.50),
    ]
    out = []
    for x, y in pts:
        out.append((x * w, h / 2 + (y - 0.5) * amp))
    return out


def app_icon(size=512):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    body = Image.new("RGBA", (size, size), BG)
    d = ImageDraw.Draw(body)
    stroke = max(10, size // 26)
    d.line(pulse_points(size, size, size * 0.62), fill=LINE, width=stroke, joint="curve")
    for i, (x, y) in enumerate(pulse_points(size, size, size * 0.62)):
        if i in (4, 5):
            continue
        r = stroke / 2
        d.ellipse([x - r, y - r, x + r, y + r], fill=LINE)
    mask = rounded_mask(size, int(size * 0.225))
    img.paste(body, (0, 0), mask)
    return img


def tray_icon(size=64):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    stroke = max(2, size // 12)
    pts = pulse_points(size, size, size * 0.66)
    d.line(pts, fill=(12, 14, 16, 220), width=stroke + 2, joint="curve")
    d.line(pts, fill=LINE, width=stroke, joint="curve")
    return img


app = app_icon(1024)
app.resize((512, 512), Image.LANCZOS).save(os.path.join(OUT, "icon.png"))
app.resize((256, 256), Image.LANCZOS).save(os.path.join(OUT, "icon-share.png"))
tray_icon(64).save(os.path.join(OUT, "tray.png"))

ico_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
app.save(
    os.path.join(OUT, "icon.ico"),
    format="ICO",
    sizes=ico_sizes,
    append_images=[app.resize(s, Image.LANCZOS) for s in ico_sizes],
)
print("icons written to", OUT, os.listdir(OUT))
