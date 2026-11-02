"""ARAM Mayhem overlay: live augment tiers on the pick cards + one-shot build import.

Windows only. Reads the League client's local API (champ select, item sets,
spells) and the in-game Live Client Data API (level, death state). Augment
cards are read off the screen with the Windows built-in OCR engine; nothing
touches the game process. Stats come from arammeta.com.

    pythonw aram_overlay.py               normal use
    python  aram_overlay.py --demo Annie  panel + card scanning without a game
    python  aram_overlay.py --scan Annie  read the screen once and print matches
    python  aram_overlay.py --print Annie dump the data as text
"""
from __future__ import annotations

import base64
import difflib
import json
import logging
import os
import queue
import re
import ssl
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

APP_DIR = Path(__file__).resolve().parent
DATA_DIR = Path(os.environ.get("LOCALAPPDATA") or Path.home()) / "AramOverlay"
TAG = "[Mayhem]"  # prefix for item sets we own; replaced on each import

DEFAULTS = {
    "league_dir": r"C:\Riot Games\League of Legends",
    "data_url": "https://arammeta.com/api",
    "cache_hours": 12,
    "toggle_hotkey": "F8",
    "quit_hotkey": "ctrl+shift+F8",
    "show_in_champ_select": True,
    "import_item_set": True,
    "set_summoner_spells": True,
    "only_mayhem": True,
    "mayhem_queue_ids": [2400],
    # tiers: win-rate gain vs the champion's average (after sample-size smoothing)
    "tier_cutoffs": {"S+": 0.035, "S": 0.02, "A": 0.008, "B": -0.01, "C": -0.025, "D": -0.04},
    "smoothing_games": 300,
    # card scanning
    "scan_cards": True,
    "ocr_language": "en-US",
    "scan_interval_fast": 0.35,  # dead / game start: when picks can be made
    "scan_interval_slow": 2.5,   # alive (0 = don't scan while alive)
    "badge_scale": 1.0,
    "hide_from_capture": True,  # false = overlay shows up in OBS/screenshots
    # fallback when OCR is unavailable: pop the panel up at augment levels
    "augment_levels": [3, 7, 11, 15],
    "auto_show_seconds": 25,
    # panel look
    "per_rarity": 8,
    "avoid_per_rarity": 3,
    "x": -24,  # negative = distance from right edge of the screen
    "y": 140,
    "opacity": 0.93,
    "font_size": 10,
}

log = logging.getLogger("aram")


def load_config() -> dict:
    cfg = dict(DEFAULTS)
    path = APP_DIR / "config.json"
    if path.exists():
        try:
            cfg.update(json.loads(path.read_text("utf-8")))
        except ValueError as e:
            log.error("config.json is not valid JSON (%s); using defaults", e)
    return cfg


# --------------------------------------------------------------------- HTTP

_LOCAL_TLS = ssl.create_default_context()
_LOCAL_TLS.check_hostname = False
_LOCAL_TLS.verify_mode = ssl.CERT_NONE  # League's local APIs use a self-signed cert


def _request(url, method="GET", body=None, headers=None, timeout=3.0, local=True):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    if data is not None:
        req.add_header("Content-Type", "application/json")
    ctx = _LOCAL_TLS if local else None
    with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
        return r.read()


def live(path: str):
    """In-game Live Client Data API; None when no game is running."""
    try:
        return json.loads(_request(f"https://127.0.0.1:2999/liveclientdata/{path}", timeout=1.0))
    except (OSError, ValueError):
        return None


class LCU:
    """League client API, authenticated from the client's lockfile."""

    def __init__(self, league_dir: str):
        self.lockfile = Path(league_dir) / "lockfile"
        self._stamp = None
        self.base = self.headers = None

    def _ready(self) -> bool:
        try:
            stamp = self.lockfile.stat().st_mtime
        except OSError:
            self._stamp = self.base = None
            return False
        if stamp != self._stamp:
            try:
                _, _, port, password, proto = self.lockfile.read_text().strip().split(":")
            except (OSError, ValueError):
                return False
            token = base64.b64encode(f"riot:{password}".encode()).decode()
            self.base = f"{proto}://127.0.0.1:{port}"
            self.headers = {"Authorization": f"Basic {token}", "Accept": "application/json"}
            self._stamp = stamp
        return True

    def get(self, path):
        if not self._ready():
            return None
        try:
            raw = _request(self.base + path, headers=self.headers)
            return json.loads(raw) if raw else None
        except (OSError, ValueError):
            return None

    def send(self, method, path, body) -> bool:
        if not self._ready():
            return False
        try:
            _request(self.base + path, method, body, self.headers)
            return True
        except urllib.error.HTTPError as e:
            log.warning("%s %s -> %s %s", method, path, e.code, e.read()[:300])
        except OSError as e:
            log.warning("%s %s failed: %s", method, path, e)
        return False


# --------------------------------------------------------------------- data

class Data:
    """arammeta.com JSON with an on-disk cache (refreshed every cache_hours)."""

    def __init__(self, base_url: str, cache_hours: float):
        self.base = base_url.rstrip("/")
        self.max_age = cache_hours * 3600
        self.dir = DATA_DIR / "cache"
        self.dir.mkdir(parents=True, exist_ok=True)
        self._meta = None

    def _get(self, rel: str, newer_than: float = 0.0):
        path = self.dir / rel.replace("/", "_")
        if path.exists():
            mtime = path.stat().st_mtime
            if time.time() - mtime < self.max_age and mtime >= newer_than:
                return json.loads(path.read_text("utf-8"))
        try:
            raw = _request(f"{self.base}/{rel}", headers={"User-Agent": "aram-overlay/1.0"},
                           timeout=15, local=False)
            data = json.loads(raw)
            path.write_bytes(raw)
            return data
        except (OSError, ValueError) as e:
            if path.exists():
                log.warning("refresh of %s failed (%s); using cached copy", rel, e)
                return json.loads(path.read_text("utf-8"))
            raise

    def meta(self) -> dict:
        path = self.dir / "tier-list.json"
        fresh = path.exists() and time.time() - path.stat().st_mtime < self.max_age
        if self._meta is None or not fresh:
            self._meta = self._get("tier-list.json")
        return self._meta

    def champ(self, cid: int) -> dict:
        meta_file = self.dir / "tier-list.json"
        # A champion file older than the tier list belongs to an older dataset.
        newer = meta_file.stat().st_mtime - 60 if meta_file.exists() else 0.0
        return self._get(f"champions/{cid}.json", newer_than=newer)

    def find_champ(self, name: str) -> int | None:
        key = norm(name)
        for cid, c in self.meta()["champs"].items():
            if key in (norm(c.get("alias", "")), norm(c.get("name_en", ""))):
                return int(cid)
        return None


def norm(text: str) -> str:
    return re.sub(r"[^a-z0-9]", "", text.lower())


# Screen rectangles the card scanner ignores: our own side panel lists augment names.
OWN_WINDOWS: list[tuple[int, int, int, int]] = []

TIERS = ["S+", "S", "A", "B", "C", "D", "F"]  # best to worst; anything below D's cutoff is F
TIER_COLORS = {"S+": "#ff4fa3", "S": "#ff6b3d", "A": "#f5c518", "B": "#8ec441",
               "C": "#3aa0ff", "D": "#8a909a", "F": "#c43c3c"}
RARITIES = [("kPrismatic", "Prismatic", "#e3b8ff"),
            ("kGold", "Gold", "#f5c518"),
            ("kSilver", "Silver", "#c3cad4")]


def champ_tiers(meta: dict, champ: dict, cid: int, cfg: dict) -> dict[int, dict]:
    """Tier every augment for this champion from its win rate vs the champion's average.

    Small samples are pulled toward the average (smoothing_games of "average" games)
    so a 70% win rate over 40 games doesn't come out as S tier.
    """
    cut = {**DEFAULTS["tier_cutoffs"], **cfg["tier_cutoffs"]}
    k = cfg["smoothing_games"]
    base = meta["champs"].get(str(cid), {}).get("wr") or 0.5

    def tier(delta):
        for t in TIERS[:-1]:
            if delta >= cut[t]:
                return t
        return "F"

    out = {}
    for a in champ.get("poolAugments", []):
        g, wr = a.get("g", 0), a.get("wr", base)
        adj = (wr * g + base * k) / (g + k)
        out[a["id"]] = {"tier": tier(adj - base), "wr": wr, "games": g, "delta": adj - base}
    for aid, a in meta["augs"].items():  # not in this champion's data: use all-champion stats
        if int(aid) not in out and a.get("wr") is not None:
            g, wr = float(a.get("g") or 0), float(a["wr"])
            adj = (wr * g + 0.5 * k) / (g + k)
            out[int(aid)] = {"tier": tier(adj - 0.5), "wr": wr, "games": int(g),
                             "delta": adj - 0.5, "global": True}
    return out


def build_view(meta: dict, champ: dict, cid: int, cfg: dict, tiers: dict) -> dict:
    augs, lut = meta["augs"], meta.get("itemLut", {})
    info = meta["champs"].get(str(cid), {})

    def rows(entries, n):
        out = []
        for a in entries:
            aug = augs.get(str(a["id"]))
            if aug:
                out.append({"name": aug.get("name_en") or aug.get("name"), "wr": a["wr"],
                            "tier": tiers.get(a["id"], {}).get("tier", "?"),
                            "games": a.get("g", 0)})
            if len(out) >= n:
                break
        return out

    def item_name(i):
        return lut.get(str(i), {}).get("e") or str(i)

    builds = []
    for g in champ.get("itemClusters", {}).get("groups", [])[:2]:
        builds.append({"name": g.get("name_en", ""), "wr": g.get("wr"),
                       "core": [item_name(x["id"]) for x in g["core"]],
                       "next": [item_name(x["id"]) for x in g.get("options", [])[:2]]})
    spells = champ.get("spells", {}).get("top", [])
    boots = champ.get("boots", {}).get("top", [])
    return {
        "cid": cid,
        "champion": info.get("name_en", f"#{cid}"),
        "patch": meta.get("patch_prefix", "?"),
        "wr": info.get("wr"),
        "games": info.get("g"),
        "cols": [{"title": title, "color": color,
                  "top": rows(champ.get("top", {}).get(key, []), cfg["per_rarity"]),
                  "avoid": rows(champ.get("bot", {}).get(key, []), cfg["avoid_per_rarity"])}
                 for key, title, color in RARITIES],
        "builds": builds,
        "spells": spells[0]["name_en"] if spells else None,
        "boots": boots[0]["name_en"] if boots else None,
    }


def make_item_set(meta: dict, champ: dict, cid: int) -> dict:
    name = meta["champs"].get(str(cid), {}).get("name_en", str(cid))

    def block(title, ids):
        seen, items = set(), []
        for i in ids:
            if i not in seen:
                seen.add(i)
                items.append({"id": str(i), "count": 1})
        return {"type": title, "items": items}

    blocks = []
    for g in champ.get("itemClusters", {}).get("groups", [])[:3]:
        label = g.get("name_en") or "Build"
        wr = f" ({g['wr'] * 100:.1f}% WR)" if g.get("wr") else ""
        blocks.append(block(f"{label} - core{wr}", [x["id"] for x in g["core"]]))
        blocks.append(block(f"{label} - next",
                            [x["id"] for x in g.get("options", []) + g.get("tail", [])]))
    boots = [b["items"][0]["id"] for b in champ.get("boots", {}).get("top", []) if b.get("items")]
    if boots:
        blocks.insert(0, block("Boots", boots))
    singles = [s["items"][0]["id"] for s in champ.get("singleItems", {}).get("top", [])
               if s.get("items")]
    if singles:
        blocks.append(block("Highest win-rate items", singles[:10]))
    return {
        "title": f"{TAG} {name} {meta.get('patch_prefix', '')}".strip(),
        "associatedChampions": [cid], "associatedMaps": [],
        "blocks": [b for b in blocks if b["items"]],
        "map": "any", "mode": "any", "type": "custom", "sortrank": 0,
        "startedFrom": "blank", "preferredItemSlots": [], "uid": str(uuid.uuid4()),
    }


def pick_spell_slots(current: tuple, want: list) -> tuple | None:
    """Order the recommended spells, keeping one already in its slot. None = no change."""
    a, b = current
    if len(want) != 2 or {a, b} == set(want):
        return None
    if b in want:
        return (want[0] if want[1] == b else want[1], b)
    first = a if a in want else want[0]
    return (first, want[1] if want[0] == first else want[0])


# ------------------------------------------------------------ card matching

class Matcher:
    """Finds augment names in OCR output (tolerates small misreads)."""

    def __init__(self, meta: dict):
        self.names: dict[str, int] = {}
        for aid, a in meta["augs"].items():
            if a.get("name_en"):
                self.names.setdefault(norm(a["name_en"]), int(aid))
        self.keys = list(self.names)

    def lookup(self, text: str) -> int | None:
        key = norm(text)
        if key in self.names:
            return self.names[key]
        if len(key) < 7:  # short names must match exactly ("POWER", "BEES!!!")
            return None
        close = difflib.get_close_matches(key, self.keys, n=1, cutoff=0.85)
        return self.names[close[0]] if close else None

    def find(self, lines: list[list[tuple]]) -> list[dict]:
        """lines: [[(word, x, y, w, h), ...], ...] in screen coordinates."""
        hits: dict[int, dict] = {}
        own = list(OWN_WINDOWS)
        for words in lines:
            cx, cy = words[0][1], words[0][2]
            if any(x <= cx <= x + w and y <= cy <= y + h for x, y, w, h in own):
                continue
            n = len(words)
            # whole line first, then sub-spans (OCR can merge neighbouring card titles)
            spans = [(0, n)] + [(i, j) for i in range(n) for j in range(i + 1, min(n, i + 4) + 1)
                                if (i, j) != (0, n)]
            used = set()
            for i, j in spans:
                if used & set(range(i, j)):
                    continue
                text = " ".join(w[0] for w in words[i:j])
                aid = self.names.get(norm(text)) if (i, j) != (0, n) else self.lookup(text)
                if aid is None:
                    continue
                used |= set(range(i, j))
                x0 = min(w[1] for w in words[i:j])
                y0 = min(w[2] for w in words[i:j])
                x1 = max(w[1] + w[3] for w in words[i:j])
                y1 = max(w[2] + w[4] for w in words[i:j])
                if aid not in hits or (y1 - y0) > hits[aid]["h"]:
                    hits[aid] = {"id": aid, "x": x0, "y": y0, "w": x1 - x0, "h": y1 - y0}
        found = list(hits.values())
        if len(found) <= 1:
            return found
        # Real card titles share a baseline and size; keep the biggest such row.
        rows = []
        for f in sorted(found, key=lambda f: -f["h"]):
            for row in rows:
                ref = row[0]
                if abs(f["y"] - ref["y"]) < ref["h"] * 1.2 and 0.6 < f["h"] / ref["h"] < 1.6:
                    row.append(f)
                    break
            else:
                rows.append([f])
        return max(rows, key=len)[:4]


# ------------------------------------------------------------ screen reading

def win32():
    import ctypes
    from ctypes import wintypes
    return ctypes, wintypes, ctypes.windll.user32


class ScreenReader:
    """GDI screen capture + Windows.Media.Ocr. Construct and use on one thread."""

    def __init__(self, language: str):
        import asyncio
        from winrt.windows.globalization import Language
        from winrt.windows.graphics.imaging import BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap
        from winrt.windows.media.ocr import OcrEngine
        from winrt.windows.storage.streams import Buffer
        self._Buffer, self._SoftwareBitmap = Buffer, SoftwareBitmap
        self._fmt, self._alpha = BitmapPixelFormat.BGRA8, BitmapAlphaMode.IGNORE
        self.engine = (OcrEngine.try_create_from_language(Language(language))
                       or OcrEngine.try_create_from_user_profile_languages())
        if self.engine is None:
            raise RuntimeError(f"no Windows OCR language pack for {language}")
        self.loop = asyncio.new_event_loop()

        ctypes, wintypes, user32 = win32()
        self.ct, self.user32, self.gdi32 = ctypes, user32, ctypes.windll.gdi32
        vp, i = ctypes.c_void_p, ctypes.c_int
        user32.GetDC.restype = vp
        user32.GetDC.argtypes = [vp]
        user32.ReleaseDC.argtypes = [vp, vp]
        g = self.gdi32
        g.CreateCompatibleDC.restype = vp
        g.CreateCompatibleDC.argtypes = [vp]
        g.CreateCompatibleBitmap.restype = vp
        g.CreateCompatibleBitmap.argtypes = [vp, i, i]
        g.SelectObject.restype = vp
        g.SelectObject.argtypes = [vp, vp]
        g.BitBlt.argtypes = [vp, i, i, i, i, vp, i, i, wintypes.DWORD]
        g.GetDIBits.argtypes = [vp, vp, wintypes.UINT, wintypes.UINT, vp, vp, wintypes.UINT]
        g.DeleteObject.argtypes = [vp]
        g.DeleteDC.argtypes = [vp]
        self.screen = (user32.GetSystemMetrics(0), user32.GetSystemMetrics(1))

    def grab(self, x: int, y: int, w: int, h: int) -> bytes:
        """BGRA pixels of a screen rectangle. Layered windows (our overlay) are not captured."""
        ct, g = self.ct, self.gdi32
        hdc = self.user32.GetDC(None)
        mdc = g.CreateCompatibleDC(hdc)
        bmp = g.CreateCompatibleBitmap(hdc, w, h)
        old = g.SelectObject(mdc, bmp)
        try:
            g.BitBlt(mdc, 0, 0, w, h, hdc, x, y, 0x00CC0020)  # SRCCOPY, no CAPTUREBLT
            header = (ct.c_uint32 * 11)(40, w & 0xFFFFFFFF, (-h) & 0xFFFFFFFF, 1 | (32 << 16), 0, 0, 0, 0, 0, 0)
            buf = ct.create_string_buffer(w * h * 4)
            g.GetDIBits(mdc, bmp, 0, h, buf, header, 0)
            return buf.raw
        finally:
            g.SelectObject(mdc, old)
            g.DeleteObject(bmp)
            g.DeleteDC(mdc)
            self.user32.ReleaseDC(None, hdc)

    def read(self, x: int, y: int, w: int, h: int) -> list[list[tuple]]:
        """OCR a screen rectangle; returns lines of (word, x, y, w, h) in screen coords."""
        x, y, w, h = int(x), int(y), int(w), int(h)
        raw = self.grab(x, y, w, h)
        buf = self._Buffer(len(raw))
        buf.length = len(raw)
        memoryview(buf)[:] = raw
        bmp = self._SoftwareBitmap.create_copy_from_buffer(buf, self._fmt, w, h, self._alpha)
        result = self.loop.run_until_complete(self.engine.recognize_async(bmp))
        lines = []
        for line in result.lines:
            words = []
            for word in line.words:
                r = word.bounding_rect
                words.append((word.text, x + r.x, y + r.y, r.width, r.height))
            if words:
                lines.append(words)
        return lines


class CardScanner(threading.Thread):
    """Finds the augment cards on screen and emits tier badges for them.

    Cards are scanned in horizontal strips: whole-card crops confuse the OCR
    (it reads the card frames as giant letters). Once the titles are found,
    that strip is remembered per resolution so later scans read ~10 ms of pixels.
    """

    def __init__(self, cfg: dict, events: queue.Queue):
        super().__init__(daemon=True)
        self.cfg, self.events = cfg, events
        self.mode = "off"  # off | slow | fast, set by the Worker
        self.matcher: Matcher | None = None
        self.tiers: dict[int, dict] = {}
        self.names: dict[int, str] = {}
        self.ready = threading.Event()
        self.error: str | None = None
        self.layout_file = DATA_DIR / "layout.json"

    def set_champion(self, meta: dict, tiers: dict):
        self.matcher = Matcher(meta)
        self.names = {int(k): v.get("name_en", k) for k, v in meta["augs"].items()}
        self.tiers = tiers

    def _load_band(self, screen):
        try:
            return tuple(json.loads(self.layout_file.read_text())[f"{screen[0]}x{screen[1]}"])
        except (OSError, ValueError, KeyError, TypeError):
            return None

    def _save_band(self, screen, band):
        try:
            layouts = json.loads(self.layout_file.read_text()) if self.layout_file.exists() else {}
            layouts[f"{screen[0]}x{screen[1]}"] = list(band)
            self.layout_file.write_text(json.dumps(layouts))
        except (OSError, ValueError):
            pass

    def sweep_bands(self):
        sw, sh = self.reader.screen
        h, step = int(sh * 0.10), int(sh * 0.05)
        x0, x1 = int(sw * 0.08), int(sw * 0.92)
        return [(x0, y, x1 - x0, h) for y in range(int(sh * 0.08), int(sh * 0.78), step)]

    def scan(self, full: bool) -> list[dict]:
        sw, _ = self.reader.screen
        if not full and self.band:
            x0 = int(sw * 0.08)
            return self.matcher.find(self.reader.read(x0, self.band[0], int(sw * 0.84),
                                                      self.band[1] - self.band[0]))
        for rect in self.sweep_bands():
            found = self.matcher.find(self.reader.read(*rect))
            if len(found) >= 2:
                top = min(f["y"] for f in found)
                bottom = max(f["y"] + f["h"] for f in found)
                pad = int((bottom - top) * 1.5) + 10
                self.band = (int(max(0, top - pad)), int(bottom + pad))
                self._save_band(self.reader.screen, self.band)
                log.info("card titles found at y=%s; scanning strip %s from now on", top, self.band)
                return found
        return []

    def run(self):
        try:
            self.reader = ScreenReader(self.cfg["ocr_language"])
        except Exception as e:  # missing winrt packages / OCR language
            self.error = str(e)
            log.warning("card scanning unavailable: %s", e)
            self.ready.set()
            return
        self.band = self._load_band(self.reader.screen)
        self.ready.set()
        shown_key, misses, last_full = None, 0, 0.0
        while True:
            mode = self.mode
            interval = (self.cfg["scan_interval_fast"] if mode == "fast"
                        else self.cfg["scan_interval_slow"])
            if mode == "off" or not self.matcher or interval <= 0:
                if shown_key:
                    self.events.put(("badges", []))
                    shown_key = None
                time.sleep(0.5)
                continue
            t0 = time.time()
            try:
                # a full sweep costs ~15 ms per strip, so only fall back to it now and then
                full = self.band is None and t0 - last_full > 2.0 or misses >= 6 and t0 - last_full > 3.0
                if full:
                    last_full = t0
                found = self.scan(full) if full or self.band else []
            except Exception:
                log.exception("scan failed")
                found = []
            if found:
                misses = 0
                key = tuple(sorted((f["id"], f["x"] // 20, f["y"] // 20) for f in found))
                if key != shown_key:
                    shown_key = key
                    self.events.put(("badges", self.badges(found)))
            else:
                misses += 1
                if shown_key and misses >= 2:
                    log.debug("cards gone")
                    shown_key = None
                    self.events.put(("badges", []))
            time.sleep(max(0.05, interval - (time.time() - t0)))

    def badges(self, found: list[dict]) -> list[dict]:
        out = []
        for f in found:
            t = self.tiers.get(f["id"], {"tier": "?", "wr": None})
            out.append({**f, "tier": t["tier"], "wr": t.get("wr"), "delta": t.get("delta"),
                        "name": self.names.get(f["id"]), "global": t.get("global", False)})
        ranked = sorted((b for b in out if b["delta"] is not None), key=lambda b: b["delta"])
        if len(ranked) >= 2 and ranked[-1]["delta"] - ranked[0]["delta"] > 0.002:
            ranked[-1]["tag"] = "BEST"
            ranked[0]["tag"] = "WORST"
        log.info("cards: %s", ", ".join(f"{b['name']} {b['tier']}" for b in out))
        return out


# ------------------------------------------------------------------- worker

class Worker(threading.Thread):
    """Polls the client/game and pushes events to the UI thread via a queue."""

    def __init__(self, cfg: dict, data: Data, events: queue.Queue, scanner: CardScanner | None):
        super().__init__(daemon=True)
        self.cfg, self.data, self.events, self.scanner = cfg, data, events, scanner
        self.lcu = LCU(cfg["league_dir"])
        self.me_name = None
        self.reset()

    def reset(self):
        self.phase = None
        self.cid = None
        self.level = None
        self.mayhem = None
        self.game_seen = False
        self.set_scan("off")

    def set_scan(self, mode):
        if self.scanner:
            self.scanner.mode = mode

    def emit(self, *event):
        self.events.put(event)

    def run(self):
        while True:
            try:
                self.tick()
            except Exception:
                log.exception("poll failed")
            time.sleep(1.0)

    def tick(self):
        phase = self.lcu.get("/lol-gameflow/v1/gameflow-phase")
        if phase != self.phase:
            log.info("phase %s -> %s", self.phase, phase)
            if phase in ("None", "Lobby", "Matchmaking", "ReadyCheck", "EndOfGame"):
                self.reset()
                self.emit("hide")
            self.phase = phase
            if phase == "InProgress":
                self.emit("hide")

        if phase in ("ChampSelect", "InProgress", "GameStart") and self.mayhem is None:
            session = self.lcu.get("/lol-gameflow/v1/session") or {}
            q = session.get("gameData", {}).get("queue", {})
            if q:
                self.mayhem = (q.get("id") in self.cfg["mayhem_queue_ids"]
                               or "KIWI" in str(q.get("gameMode", "")).upper())
                log.info("queue %s (%s) mayhem=%s", q.get("id"), q.get("gameMode"), self.mayhem)
        if self.cfg["only_mayhem"] and self.mayhem is False:
            return

        if phase == "ChampSelect":
            self.champ_select()
        elif phase == "InProgress" or phase is None:
            self.in_game()

    def champ_select(self):
        s = self.lcu.get("/lol-champ-select/v1/session")
        if not s:
            return
        cell = s.get("localPlayerCellId")
        me = next((p for p in s.get("myTeam", []) if p.get("cellId") == cell), None)
        cid = (me or {}).get("championId") or 0
        if cid and cid != self.cid:
            self.cid = cid
            self.load(cid, show=self.cfg["show_in_champ_select"], me=me)

    def load(self, cid: int, show: bool, me: dict | None = None):
        meta, champ = self.data.meta(), self.data.champ(cid)
        tiers = champ_tiers(meta, champ, cid, self.cfg)
        if self.scanner:
            self.scanner.set_champion(meta, tiers)
        self.emit("view", build_view(meta, champ, cid, self.cfg, tiers))
        if show:
            self.emit("show")
        if me is None:
            return
        notes = []
        if self.cfg["import_item_set"]:
            notes.append("item set ✓" if self.import_item_set(make_item_set(meta, champ, cid))
                         else "item set ✗")
        want = [s["id"] for s in (champ.get("spells", {}).get("top") or [{}])[0].get("items", [])]
        if self.cfg["set_summoner_spells"] and want:
            slots = pick_spell_slots((me.get("spell1Id"), me.get("spell2Id")), want)
            if slots is None:
                notes.append("spells ✓")
            else:
                ok = self.lcu.send("PATCH", "/lol-champ-select/v1/session/my-selection",
                                   {"spell1Id": slots[0], "spell2Id": slots[1]})
                notes.append("spells ✓" if ok else "spells ✗")
        if notes:
            self.emit("status", "Imported: " + ", ".join(notes))

    def import_item_set(self, item_set: dict) -> bool:
        me = self.lcu.get("/lol-summoner/v1/current-summoner")
        if not me:
            return False
        sid = me["summonerId"]
        current = self.lcu.get(f"/lol-item-sets/v1/item-sets/{sid}/sets") or {}
        sets = [s for s in current.get("itemSets", []) if not s.get("title", "").startswith(TAG)]
        sets.append(item_set)
        body = {"accountId": me.get("accountId", current.get("accountId")),
                "itemSets": sets, "timestamp": int(time.time() * 1000)}
        return self.lcu.send("PUT", f"/lol-item-sets/v1/item-sets/{sid}/sets", body)

    def in_game(self):
        name = live("activeplayername")
        if not name:
            self.set_scan("off")
            return
        players = live("playerlist") or []
        mine = next((p for p in players if name in (p.get("riotId"), p.get("summonerName"))), None)
        if self.cid is None:  # started mid-game: identify our champion from the live API
            raw = (mine or {}).get("rawChampionName", "").rsplit("_", 1)[-1]
            cid = self.data.find_champ(raw) if raw else None
            if cid is None:
                return
            self.cid = cid
            self.load(cid, show=False)

        # Augments can be picked at game start and while dead: scan fast then.
        game_time = (live("gamestats") or {}).get("gameTime", 0)
        dead = bool((mine or {}).get("isDead"))
        self.set_scan("fast" if dead or game_time < 150 else "slow")

        if self.scanner and self.scanner.error is None:
            return  # cards get badges directly; no need to pop the panel
        level = (live("activeplayer") or {}).get("level")
        if not level:
            return
        if not self.game_seen:
            self.game_seen = True
            self.emit("flash", self.cfg["auto_show_seconds"])
        elif level != self.level and level in self.cfg["augment_levels"]:
            self.emit("flash", self.cfg["auto_show_seconds"])
        self.level = level


# ----------------------------------------------------------------- Windows UI

def parse_hotkey(text: str) -> tuple[int, int]:
    mods = {"alt": 0x1, "ctrl": 0x2, "control": 0x2, "shift": 0x4, "win": 0x8}
    special = {"space": 0x20, "tab": 0x09, "insert": 0x2D, "home": 0x24, "end": 0x23,
               "pageup": 0x21, "pagedown": 0x22, "`": 0xC0}
    *mod_parts, key = [p.strip().lower() for p in text.split("+")]
    mod = sum(mods[m] for m in mod_parts)
    if key.startswith("f") and key[1:].isdigit():
        vk = 0x6F + int(key[1:])
    elif key in special:
        vk = special[key]
    elif len(key) == 1:
        vk = ord(key.upper())
    else:
        raise ValueError(f"unknown hotkey: {text}")
    return mod | 0x4000, vk  # MOD_NOREPEAT


def hotkey_thread(bindings: list[tuple[str, str]], events: queue.Queue):
    def loop():
        ctypes, wintypes, user32 = win32()
        for i, (combo, action) in enumerate(bindings, 1):
            mod, vk = parse_hotkey(combo)
            if not user32.RegisterHotKey(None, i, mod, vk):
                log.error("could not register hotkey %s (in use by another app?)", combo)
                events.put(("status", f"Hotkey {combo} unavailable - change it in config.json"))
        msg = wintypes.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            if msg.message == 0x0312:  # WM_HOTKEY
                events.put(("hotkey", bindings[msg.wParam - 1][1]))
    threading.Thread(target=loop, daemon=True).start()


BG, FG, DIM = "#10141b", "#e8ecf1", "#7d8794"


class OverlayWindow:
    """A borderless, always-on-top, click-through window that never takes focus."""

    hide_from_capture = True

    def __init__(self, tk_window, opacity: float):
        ctypes, _, user32 = win32()
        self.win, self.user32 = tk_window, user32
        w = tk_window
        w.overrideredirect(True)
        w.attributes("-topmost", True)
        w.configure(bg=BG)
        w.update()
        self.hwnd = user32.GetParent(w.winfo_id()) or w.winfo_id()
        # layered + click-through + no taskbar button + never activates
        style = user32.GetWindowLongW(self.hwnd, -20)
        user32.SetWindowLongW(self.hwnd, -20, style | 0x80000 | 0x20 | 0x80 | 0x08000000)
        w.attributes("-alpha", opacity)
        # keep our own text out of screen captures so the card scanner never reads it
        if self.hide_from_capture:
            user32.SetWindowDisplayAffinity(self.hwnd, 0x11)  # WDA_EXCLUDEFROMCAPTURE (Win10 2004+)
        self.visible = False
        self.hide()

    # Raw Win32 show/hide so the game never loses focus.
    def show(self):
        self.user32.ShowWindow(self.hwnd, 4)  # SW_SHOWNOACTIVATE
        self.raise_()
        self.visible = True

    def hide(self):
        self.user32.ShowWindow(self.hwnd, 0)
        self.visible = False

    def move(self, x: int, y: int):
        # Tk's geometry() doesn't reliably move a window we hide/show via Win32
        self.user32.SetWindowPos(self.hwnd, -1, int(x), int(y), 0, 0, 0x1 | 0x10)  # NOSIZE|NOACTIVATE

    def raise_(self):
        self.user32.SetWindowPos(self.hwnd, -1, 0, 0, 0, 0, 0x1 | 0x2 | 0x10)  # TOPMOST, no move/size/activate


class Overlay:
    def __init__(self, cfg: dict, events: queue.Queue):
        import tkinter as tk
        self.tk, self.cfg, self.events = tk, cfg, events
        self.root = tk.Tk()
        self.root.title("ARAM Mayhem Overlay")
        self.f = cfg["font_size"]
        self.body = tk.Frame(self.root, bg=BG, padx=12, pady=8,
                             highlightthickness=1, highlightbackground="#2a3340")
        self.body.pack()
        self.status = "Waiting for champ select…"
        self.view = None
        self.hide_job = None
        OverlayWindow.hide_from_capture = cfg["hide_from_capture"]
        self.panel = OverlayWindow(self.root, cfg["opacity"])
        self.render()
        self.badges: list[tuple] = []  # (Toplevel, OverlayWindow)
        self.root.after(100, self.pump)
        self.root.after(2000, self.keep_on_top)

    def keep_on_top(self):
        for w in [self.panel] + [b for _, b in self.badges]:
            if w.visible:
                w.raise_()
        self.root.after(2000, self.keep_on_top)

    def _cancel_hide(self):
        if self.hide_job:
            self.root.after_cancel(self.hide_job)
            self.hide_job = None

    def pump(self):
        try:
            while True:
                kind, *args = self.events.get_nowait()
                if kind == "view":
                    self.view = args[0]
                    self.render()
                elif kind == "status":
                    self.status = args[0]
                    self.render()
                elif kind == "badges":
                    self.show_badges(args[0])
                elif kind == "show":
                    self._cancel_hide()
                    self.panel.show()
                elif kind == "flash":
                    self._cancel_hide()
                    self.panel.show()
                    self.hide_job = self.root.after(int(args[0] * 1000), self.panel.hide)
                elif kind == "hide":
                    self._cancel_hide()
                    self.panel.hide()
                    self.show_badges([])
                elif kind == "hotkey" and args[0] == "toggle":
                    self._cancel_hide()
                    self.panel.hide() if self.panel.visible else self.panel.show()
                elif kind == "hotkey" and args[0] == "quit":
                    self.root.destroy()
                    return
        except queue.Empty:
            pass
        r = self.root
        OWN_WINDOWS[:] = ([(r.winfo_rootx(), r.winfo_rooty(), r.winfo_width(), r.winfo_height())]
                          if self.panel.visible else [])
        self.root.after(100, self.pump)

    # ---- tier badges on the augment cards

    def show_badges(self, badges: list[dict]):
        tk = self.tk
        while len(self.badges) < len(badges):
            top = tk.Toplevel(self.root)
            self.badges.append((top, OverlayWindow(top, 0.96)))
        for i, (top, win) in enumerate(self.badges):
            if i >= len(badges):
                win.hide()
                continue
            b = badges[i]
            for child in top.winfo_children():
                child.destroy()
            px = max(14, int(b["h"] * 0.85 * self.cfg["badge_scale"]))
            color = TIER_COLORS.get(b["tier"], DIM)
            tag = b.get("tag")
            frame = tk.Frame(top, bg=BG, highlightthickness=2,
                             highlightbackground={"BEST": "#4ade80", "WORST": "#f87171"}.get(tag, color))
            frame.pack()
            tk.Label(frame, text=b["tier"], bg=color, fg="#ffffff" if b["tier"] == "F" else "#111111",
                     padx=int(px * 0.45),
                     font=("Segoe UI", -px, "bold")).pack(side="left")
            detail = f"{b['wr'] * 100:.1f}%" if b["wr"] is not None else "no data"
            if b.get("global"):
                detail += " (all)"
            tk.Label(frame, text=detail, bg=BG, fg=FG, padx=int(px * 0.4),
                     font=("Segoe UI", -int(px * 0.75), "bold")).pack(side="left", fill="y")
            if tag:
                tk.Label(frame, text=tag, bg="#1f7a45" if tag == "BEST" else "#9b2c2c", fg="#ffffff",
                         padx=int(px * 0.3), font=("Segoe UI", -int(px * 0.6), "bold")
                         ).pack(side="left", fill="y")
            top.update_idletasks()
            w, h = top.winfo_reqwidth(), top.winfo_reqheight()
            x = int(b["x"] + b["w"] / 2 - w / 2)
            y = int(b["y"] - h - b["h"] * 0.5)
            top.geometry(f"{w}x{h}+{x}+{y}")
            top.update_idletasks()
            win.move(x, y)
            win.show()
            log.debug("badge %s %s at %s,%s size %sx%s", b["name"], b["tier"], x, y, w, h)

    # ---- side panel

    def label(self, parent, text, color=FG, size=0, bold=False, **grid):
        w = self.tk.Label(parent, text=text, fg=color, bg=BG, anchor="w", justify="left",
                          font=("Segoe UI", self.f + size, "bold" if bold else "normal"))
        w.grid(**grid)
        return w

    def render(self):
        for w in self.body.winfo_children():
            w.destroy()
        v = self.view
        if v is None:
            self.label(self.body, "ARAM Mayhem Overlay", bold=True, size=1, row=0, column=0, sticky="w")
            self.label(self.body, self.status, DIM, -1, row=1, column=0, sticky="w")
            self._place()
            return

        head = self.tk.Frame(self.body, bg=BG)
        head.grid(row=0, column=0, sticky="w", pady=(0, 6))
        self.label(head, v["champion"], bold=True, size=3, row=0, column=0, sticky="w")
        sub = f"Mayhem {v['patch']}"
        if v.get("wr"):
            sub += f" · {v['wr'] * 100:.1f}% WR · {v['games'] / 1000:.0f}k games"
        self.label(head, "   " + sub, DIM, -1, row=0, column=1, sticky="sw")

        cols = self.tk.Frame(self.body, bg=BG)
        cols.grid(row=1, column=0, sticky="w")
        for c, col in enumerate(v["cols"]):
            box = self.tk.Frame(cols, bg=BG, padx=(0 if c == 0 else 14))
            box.grid(row=0, column=c, sticky="n")
            self.label(box, col["title"].upper(), col["color"], -1, True,
                       row=0, column=0, columnspan=3, sticky="w")
            for r, a in enumerate(col["top"], 1):
                self.label(box, a["tier"], TIER_COLORS.get(a["tier"], DIM), 0, True,
                           row=r, column=0, sticky="w", padx=(0, 6))
                self.label(box, a["name"][:24], row=r, column=1, sticky="w")
                self.label(box, f"{a['wr'] * 100:.1f}%", DIM, -1,
                           row=r, column=2, sticky="e", padx=(8, 0))
            if col["avoid"]:
                avoid = "\n".join(f"✗ {a['tier']}  {a['name'][:24]}" for a in col["avoid"])
                self.label(box, avoid, "#b06a6a", -2, row=len(col["top"]) + 1, column=0,
                           columnspan=3, sticky="w", pady=(4, 0))

        info = self.tk.Frame(self.body, bg=BG)
        info.grid(row=2, column=0, sticky="w", pady=(8, 0))
        r = 0
        for b in v["builds"]:
            line = " + ".join(b["core"])
            if b["next"]:
                line += "  →  " + " / ".join(b["next"])
            self.label(info, f"{b['name']}: ", DIM, -1, row=r, column=0, sticky="w")
            self.label(info, line, size=-1, row=r, column=1, sticky="w")
            r += 1
        extras = " · ".join(x for x in (v.get("spells"), v.get("boots")) if x)
        if extras:
            self.label(info, "Spells/boots: ", DIM, -1, row=r, column=0, sticky="w")
            self.label(info, extras, size=-1, row=r, column=1, sticky="w")
        hot = f"{self.cfg['toggle_hotkey']} show/hide · {self.status} · data: arammeta.com"
        self.label(self.body, hot, DIM, -2, row=3, column=0, sticky="w", pady=(6, 0))
        self._place()

    def _place(self):
        self.root.update_idletasks()
        w = self.root.winfo_reqwidth()
        x, y = self.cfg["x"], self.cfg["y"]
        if x < 0:
            x = self.root.winfo_screenwidth() - w + x
        self.root.geometry(f"+{x}+{y}")


def single_instance() -> bool:
    ctypes, _, _ = win32()
    ctypes.windll.kernel32.CreateMutexW(None, False, "AramMayhemOverlay")
    return ctypes.windll.kernel32.GetLastError() != 183  # ERROR_ALREADY_EXISTS


def main():
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    logging.basicConfig(level=logging.DEBUG if os.environ.get("ARAM_DEBUG") else logging.INFO, format="%(asctime)s %(levelname)s %(message)s",
                        handlers=[logging.FileHandler(DATA_DIR / "overlay.log", encoding="utf-8"),
                                  logging.StreamHandler()])
    cfg = load_config()
    data = Data(cfg["data_url"], cfg["cache_hours"])
    args = sys.argv[1:]
    flag, champ_name = (args[0], " ".join(args[1:]) or "Annie") if args else (None, None)

    def champ_context(name):
        cid = data.find_champ(name)
        if cid is None:
            sys.exit(f"unknown champion: {name}")
        meta, champ = data.meta(), data.champ(cid)
        tiers = champ_tiers(meta, champ, cid, cfg)
        return meta, tiers, build_view(meta, champ, cid, cfg, tiers)

    if flag == "--print":
        print(json.dumps(champ_context(champ_name)[2], indent=1, ensure_ascii=False))
        return

    if sys.platform != "win32":
        sys.exit("The overlay runs on Windows (where League runs). Use --print to test data here.")
    try:
        import ctypes
        ctypes.windll.shcore.SetProcessDpiAwareness(1)  # physical pixels: crisp text, exact badge placement
    except (AttributeError, OSError):
        pass

    if flag == "--scan":
        meta, tiers, _ = champ_context(champ_name)
        scanner = CardScanner(cfg, queue.Queue())
        scanner.reader = ScreenReader(cfg["ocr_language"])
        scanner.band = None
        scanner.set_champion(meta, tiers)
        t = time.perf_counter()
        for rect in scanner.sweep_bands():
            for words in scanner.reader.read(*rect):
                print(f"  y={int(words[0][2]):5}  {' '.join(w[0] for w in words)}")
        print(f"full sweep OCR: {(time.perf_counter() - t) * 1000:.0f} ms")
        for b in scanner.badges(scanner.scan(full=True)):
            print(f"MATCH {b['name']:24} tier {b['tier']}  {b['wr']}  at ({b['x']:.0f},{b['y']:.0f})")
        return

    if not single_instance():
        sys.exit("Already running.")
    events: queue.Queue = queue.Queue()
    overlay = Overlay(cfg, events)
    hotkey_thread([(cfg["toggle_hotkey"], "toggle"), (cfg["quit_hotkey"], "quit")], events)

    scanner = CardScanner(cfg, events) if cfg["scan_cards"] else None
    if scanner:
        scanner.start()
        scanner.ready.wait(10)
        if scanner.error:
            events.put(("status", "card scanning off: pip install -r requirements.txt"))

    if flag == "--demo":
        meta, tiers, view = champ_context(champ_name)
        if scanner:
            scanner.set_champion(meta, tiers)
            scanner.mode = "fast"
        events.put(("view", view))
        events.put(("status", "demo mode"))
        events.put(("show",))
    else:
        Worker(cfg, data, events, scanner).start()
    log.info("overlay running (toggle %s, quit %s)", cfg["toggle_hotkey"], cfg["quit_hotkey"])
    overlay.root.mainloop()


if __name__ == "__main__":
    main()
