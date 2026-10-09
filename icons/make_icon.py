#!/usr/bin/env python3
"""WhatsApp Sender simgesini üretir.

Tek kaynaktan (bu dosya) tüm boyutları çıkarır: PNG'ler, çok boyutlu .ico
(Windows) ve .icns (macOS). Yeniden üretmek için:  python3 icons/make_icon.py

Tasarım: yeşil gradyan zemin üzerinde beyaz konuşma balonu ve içinde uçak
(gönder simgesi). 32 pikselde de okunur olması için kalın ve yalın tutuldu.
"""
import math
import struct
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent

BASE = 2048          # çizim çözünürlüğü (sonra küçültülür)
MASTER = 1024        # ana kaynak kare

# Renkler
GREEN_LIGHT = (52, 214, 106)     # #34D66A
GREEN_DARK = (13, 122, 104)      # #0D7A68
BUBBLE = (255, 255, 255)
PLANE_LIGHT = (37, 211, 102)     # #25D366
PLANE_DARK = (16, 140, 126)      # #108C7E


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def rounded_square_mask(size, radius_ratio=0.225, pad_ratio=0.055):
    """Squircle benzeri yuvarlatılmış kare maskesi."""
    mask = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(mask)
    pad = size * pad_ratio
    d.rounded_rectangle(
        [pad, pad, size - pad, size - pad],
        radius=size * radius_ratio,
        fill=255,
    )
    return mask


def gradient(size, c1, c2):
    """Köşeden köşeye yumuşak gradyan."""
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(0, size, 8):          # 8 piksellik bloklar yeter
            t = (x / size * 0.45) + (y / size * 0.55)
            col = lerp(c1, c2, min(max(t, 0.0), 1.0))
            for k in range(8):
                if x + k < size:
                    px[x + k, y] = col
    return img


def transform(points, scale, angle_deg, cx, cy):
    """Yerel koordinatları ölçekle/döndür/taşı (ekran koordinatı, y aşağı)."""
    a = math.radians(angle_deg)
    ca, sa = math.cos(a), math.sin(a)
    out = []
    for x, y in points:
        x, y = x * scale, y * scale
        out.append((cx + x * ca - y * sa, cy + x * sa + y * ca))
    return out


def draw_foreground(size):
    """Beyaz konuşma balonu + içinde uçak. Şeffaf katman döner."""
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)

    # --- Konuşma balonu ---
    bw, bh = size * 0.63, size * 0.48
    bx, by = (size - bw) / 2, size * 0.25
    r = size * 0.115
    d.rounded_rectangle([bx, by, bx + bw, by + bh], radius=r, fill=BUBBLE + (255,))

    # Balonun kuyruğu (sol alt)
    tail = [
        (bx + bw * 0.14, by + bh - size * 0.012),
        (bx + bw * 0.06, by + bh + size * 0.115),
        (bx + bw * 0.38, by + bh - size * 0.012),
    ]
    d.polygon(tail, fill=BUBBLE + (255,))

    # --- Uçak (gönder) ---
    # Görsel ağırlık merkezi burnun solunda; balonun içinde nefes payı kalması
    # için hem biraz küçültüldü hem de ortalanırken hafifçe sola çekildi.
    cx, cy = size * 0.487, by + bh * 0.505
    s = size * 0.140
    # üst kanat: burun -> sol üst -> orta kıvrım
    upper = transform(
        [(1.00, 0.00), (-0.98, -0.86), (0.02, 0.16)], s, -12, cx, cy
    )
    # alt kanat: burun -> orta kıvrım -> sağ alt
    lower = transform(
        [(1.00, 0.00), (0.02, 0.16), (-0.42, 0.88)], s, -12, cx, cy
    )
    d.polygon(lower, fill=PLANE_DARK + (255,))
    d.polygon(upper, fill=PLANE_LIGHT + (255,))
    return layer


def build_master():
    size = BASE
    bg = gradient(size, GREEN_LIGHT, GREEN_DARK).convert("RGBA")
    bg.putalpha(rounded_square_mask(size))

    # Üstte yumuşak ışık
    glow = Image.new("L", (size, size), 0)
    ImageDraw.Draw(glow).ellipse(
        [size * 0.05, -size * 0.55, size * 0.95, size * 0.42], fill=46
    )
    glow = glow.filter(ImageFilter.GaussianBlur(size * 0.05))
    bg = Image.composite(Image.new("RGBA", (size, size), (255, 255, 255, 255)), bg, glow)

    fg = draw_foreground(size)
    return Image.alpha_composite(bg, fg)


def save_icns(master, path):
    """PNG tabanlı .icns (macOS 10.7+ destekler)."""
    entries = [
        (b"ic11", 32),    # 16@2x
        (b"ic12", 64),    # 32@2x
        (b"ic07", 128),
        (b"ic13", 256),   # 128@2x
        (b"ic08", 256),
        (b"ic14", 512),   # 256@2x
        (b"ic09", 512),
        (b"ic10", 1024),
    ]
    chunks = b""
    for tag, px in entries:
        buf = path.parent / f".tmp_{px}.png"
        master.resize((px, px), Image.LANCZOS).save(buf, "PNG")
        data = buf.read_bytes()
        buf.unlink()
        chunks += tag + struct.pack(">I", len(data) + 8) + data
    path.write_bytes(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)


def main():
    master = build_master().resize((MASTER, MASTER), Image.LANCZOS)

    png_targets = {
        "icon.png": 512,
        "128x128@2x.png": 256,
        "128x128.png": 128,
        "32x32.png": 32,
        "source.png": MASTER,
    }
    for name, px in png_targets.items():
        master.resize((px, px), Image.LANCZOS).save(HERE / name, "PNG")
        print("yazildi:", name, f"{px}x{px}")

    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    master.save(HERE / "icon.ico", format="ICO", sizes=[(s, s) for s in ico_sizes])
    print("yazildi: icon.ico", ico_sizes)

    save_icns(master, HERE / "icon.icns")
    print("yazildi: icon.icns (7 boyut)")


if __name__ == "__main__":
    sys.exit(main())
