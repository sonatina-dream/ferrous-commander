"""Builds the sample home folder used by scripts/demo.sh (no personal data).

Usage: python3 -I scripts/make_demo_data.py <demo-root>
Creates <demo-root>/home (files) and <demo-root>/config (settings + session).
"""

import json
import math
import os
import random
import struct
import sys
import time
import zipfile
import zlib

random.seed(7)
NOW = time.time()
DAY = 86400


def touch(path, days_ago, hours=0.0):
    t = NOW - days_ago * DAY - hours * 3600
    os.utime(path, (t, t))


def write(path, data, days_ago=0.0, hours=0.0):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(path, mode) as f:
        f.write(data)
    touch(path, days_ago, hours)


def blob(size):
    return bytes(random.getrandbits(8) for _ in range(min(size, 4096))) * (size // 4096 + 1)


def png(width, height, pixel):
    """Minimal RGB PNG encoder; pixel(x, y) -> (r, g, b)."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            r, g, b = pixel(x, y)
            rows += bytes((max(0, min(255, int(r))), max(0, min(255, int(g))), max(0, min(255, int(b)))))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b"")


def lerp(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def landscape(sky_top, sky_bottom, sun, hills, water, seed):
    rnd = random.Random(seed)
    phase = [rnd.random() * 6.28 for _ in range(6)]
    sun_x, sun_y, sun_r = sun

    def pixel(x, y, w=480, h=320):
        u, v = x / w, y / h
        horizon = 0.62
        col = lerp(sky_top, sky_bottom, min(1.0, v / horizon))
        d = math.hypot(u - sun_x, (v - sun_y) * 0.66)
        if d < sun_r:
            col = lerp(col, (255, 244, 214), 0.92)
        elif d < sun_r * 2.6:
            col = lerp(col, (255, 226, 180), 0.35 * (1 - (d - sun_r) / (sun_r * 1.6)))
        for i, (base, amp, color) in enumerate(hills):
            ridge = base + amp * (math.sin(u * (3 + i * 2) + phase[i]) * 0.6 + math.sin(u * (9 + i * 3) + phase[i + 1]) * 0.25)
            if v > ridge:
                col = lerp(color, lerp(color, (0, 0, 0), 0.35), (v - ridge) * 2)
        if water and v > 0.8:
            wave = math.sin(u * 60 + v * 40) * 0.04
            col = lerp(water, sky_bottom, 0.25 + wave)
        return col

    return png(480, 320, pixel)


def main():
    root = sys.argv[1]
    home = os.path.join(root, "home")
    config = os.path.join(root, "config")
    os.makedirs(home, exist_ok=True)
    os.makedirs(config, exist_ok=True)
    j = lambda *p: os.path.join(home, *p)

    # Projects
    site = j("Projects", "ferrous-site")
    write(os.path.join(site, "index.html"), "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title>Ferrous Site</title>\n  <link rel=\"stylesheet\" href=\"styles.css\">\n</head>\n<body>\n  <main id=\"app\"></main>\n  <script src=\"app.js\"></script>\n</body>\n</html>\n", 0, 2)
    write(os.path.join(site, "styles.css"), ":root {\n  --bg: #0b0b0c;\n  --fg: #ececef;\n}\n\nbody {\n  margin: 0;\n  font: 16px/1.5 system-ui;\n  background: var(--bg);\n  color: var(--fg);\n}\n", 0, 5)
    write(os.path.join(site, "app.js"), """// Renders a small gallery from a JSON manifest.
const app = document.querySelector('#app');

async function load() {
  const response = await fetch('assets/manifest.json');
  const photos = await response.json();
  return photos.filter((p) => p.published);
}

function card(photo) {
  const el = document.createElement('figure');
  el.innerHTML = `
    <img src="${photo.src}" alt="${photo.title}" loading="lazy">
    <figcaption>${photo.title} · ${photo.place}</figcaption>`;
  return el;
}

load()
  .then((photos) => photos.forEach((p) => app.append(card(p))))
  .catch((err) => {
    app.textContent = 'Could not load the gallery.';
    console.error(err);
  });
""", 0, 1)
    write(os.path.join(site, "README.md"), "# Ferrous Site\n\nA tiny static gallery.\n\n```sh\nnpx serve .\n```\n", 3)
    write(os.path.join(site, "package.json"), json.dumps({"name": "ferrous-site", "version": "1.4.0", "private": True, "scripts": {"start": "serve ."}}, indent=2), 6)
    write(os.path.join(site, ".gitignore"), "node_modules/\n.DS_Store\n", 40)
    write(os.path.join(site, "assets", "manifest.json"), json.dumps([{"title": "Alps sunrise", "place": "Chamonix", "src": "alps.jpg", "published": True}], indent=2), 2)
    write(os.path.join(site, "assets", "logo.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 32 32\"><circle cx=\"16\" cy=\"16\" r=\"14\"/></svg>\n", 12)
    write(os.path.join(site, "src", "router.ts"), "export type Route = { path: string; view: () => HTMLElement };\n", 4)
    write(os.path.join(site, "src", "store.ts"), "export const store = new Map<string, unknown>();\n", 4)

    cli = j("Projects", "rust-cli")
    write(os.path.join(cli, "Cargo.toml"), "[package]\nname = \"rust-cli\"\nversion = \"0.3.1\"\nedition = \"2024\"\n\n[dependencies]\nclap = \"4\"\n", 9)
    write(os.path.join(cli, "src", "main.rs"), "use std::env;\n\nfn main() {\n    let name = env::args().nth(1).unwrap_or_else(|| \"world\".into());\n    println!(\"Hello, {name}!\");\n}\n", 1, 6)
    write(os.path.join(cli, "README.md"), "# rust-cli\n\nSays hello.\n", 20)
    write(os.path.join(j("Projects", "design-system"), "tokens.json"), json.dumps({"radius": 12, "accent": "#f2f2f7"}, indent=2), 15)
    write(os.path.join(j("Projects", "design-system"), "icons.sketch"), blob(2_400_000), 30)
    write(j("Projects", "ideas.md"), "# Ideas\n\n- Dual-pane everything\n- Keyboard first\n", 1, 2)
    write(j("Projects", "roadmap.pdf"), b"%PDF-1.4\n" + blob(380_000), 11)
    write(j("Projects", "launch-video.mp4"), blob(48_000_000), 25)
    with zipfile.ZipFile(j("Projects", "brand-assets.zip"), "w") as z:
        z.writestr("brand/logo.txt", "logo")
        z.writestr("brand/colors.txt", "#000 #fff")
    touch(j("Projects", "brand-assets.zip"), 60)
    for d, days in [("ferrous-site", 0), ("rust-cli", 1), ("design-system", 15)]:
        touch(j("Projects", d), days, 1)

    # Photos
    scenes = [
        ("Alps sunrise", (64, 92, 160), (250, 170, 120), (0.72, 0.42, 0.06), [(0.55, 0.08, (60, 70, 98)), (0.66, 0.05, (34, 40, 58))], None),
        ("Lisbon dusk", (40, 34, 86), (236, 120, 110), (0.3, 0.5, 0.05), [(0.68, 0.03, (52, 40, 70)), (0.74, 0.02, (30, 24, 44))], (40, 60, 110)),
        ("Desert road", (90, 150, 220), (246, 214, 168), (0.82, 0.22, 0.05), [(0.6, 0.05, (196, 128, 82)), (0.7, 0.03, (150, 90, 58))], None),
        ("Fjord morning", (110, 170, 220), (214, 230, 240), (0.2, 0.25, 0.04), [(0.45, 0.14, (70, 92, 110)), (0.6, 0.07, (40, 58, 70))], (60, 110, 140)),
        ("Night city", (8, 10, 30), (40, 40, 90), (0.75, 0.2, 0.025), [(0.62, 0.06, (20, 22, 40)), (0.7, 0.04, (12, 12, 24))], (18, 22, 50)),
        ("Lavender field", (150, 170, 230), (250, 210, 230), (0.4, 0.38, 0.05), [(0.62, 0.02, (130, 100, 180)), (0.72, 0.02, (100, 70, 150))], None),
        ("Autumn forest", (120, 160, 210), (240, 200, 150), (0.6, 0.3, 0.05), [(0.52, 0.06, (170, 90, 40)), (0.64, 0.05, (120, 60, 30))], None),
        ("Coastline", (90, 160, 230), (200, 230, 250), (0.15, 0.3, 0.05), [(0.6, 0.04, (90, 110, 80)), (0.7, 0.02, (60, 80, 60))], (40, 120, 170)),
    ]
    for i, (name, top, bottom, sun, hills, water) in enumerate(scenes):
        write(j("Photos", f"{name}.png"), landscape(top, bottom, sun, hills, water, i), i * 3 + 0.5)
    os.makedirs(j("Photos", "Raw"), exist_ok=True)
    for k in range(4):
        write(j("Photos", "Raw", f"IMG_{2401 + k}.dng"), blob(24_000_000 + k * 1_500_000), 40 + k)
    touch(j("Photos", "Raw"), 40)

    # Documents, Downloads, Desktop, Music, Movies
    write(j("Documents", "Meeting notes.md"), "# Weekly sync\n\n- Ship the clipboard work\n- Review the light theme\n", 0, 3)
    write(j("Documents", "Budget 2026.xlsx"), b"PK" + blob(86_000), 5)
    write(j("Documents", "Contract draft.pdf"), b"%PDF-1.7\n" + blob(240_000), 12)
    write(j("Documents", "Travel plan.txt"), "Lisbon → Porto → Madrid\n", 30)
    write(os.path.join(j("Documents", "Recipes"), "Pasta.md"), "# Pasta\n", 70)
    write(j("Downloads", "release-notes.pdf"), b"%PDF-1.5\n" + blob(120_000), 2)
    write(j("Downloads", "fonts.zip"), b"PK" + blob(3_200_000), 8)
    write(j("Desktop", "Moodboard.png"), landscape((30, 30, 32), (90, 90, 96), (0.5, 0.45, 0.08), [(0.7, 0.03, (20, 20, 22))], None, 99), 1)
    for k, title in enumerate(["Lo-fi sketch", "Morning run", "Focus mix"]):
        write(j("Music", f"{title}.mp3"), blob(6_000_000 + k * 900_000), 10 + k * 4)
    os.makedirs(j("Movies"), exist_ok=True)
    with open(j("Movies", "Holiday 4K.mov"), "wb") as f:
        f.truncate(4_000_000_000)  # sparse: instant to create, real bytes when copied
    touch(j("Movies", "Holiday 4K.mov"), 3)

    settings = {
        "appearance": {"theme": "Black", "font_size": 13.0, "viewer_font_size": 13.0},
        "favorites": [j("Projects"), j("Photos"), j("Documents")],
    }
    with open(os.path.join(config, "settings.json"), "w") as f:
        json.dump(settings, f, indent=2)
    tab = lambda p: {"path": p, "mode": "full", "sort": {"key": "name", "ascending": True}, "lock": 0}
    session = {
        "left": {"tabs": [tab(j("Projects"))], "active": 0, "folder_history": [j("Documents"), j("Downloads")]},
        "right": {"tabs": [tab(j("Photos"))], "active": 0, "folder_history": []},
        "active_side": 0,
        "split": 0.5,
        "command_history": [],
        "window": {"x": 120.0, "y": 80.0, "width": 1280.0, "height": 800.0},
    }
    with open(os.path.join(config, "session.json"), "w") as f:
        json.dump(session, f, indent=2)


if __name__ == "__main__":
    main()
