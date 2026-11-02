"""ARAM Mayhem overlay: best augments for your champion + one-shot build import.

Windows only, Python standard library only. Reads the League client's local
API (champ select, item sets, spells) and the in-game Live Client Data API
(your level). Never touches the game process. Stats come from arammeta.com.

    pythonw aram_overlay.py              normal use
    python  aram_overlay.py --demo Annie preview the overlay without League
    python  aram_overlay.py --print Annie dump the data as text
"""
from __future__ import annotations

import base64
import json
import logging
import os
import queue
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
    "augment_levels": [3, 7, 11, 15],
    "auto_show_seconds": 25,
    "show_in_champ_select": True,
    "import_item_set": True,
    "set_summoner_spells": True,
    "only_mayhem": True,
    "mayhem_queue_ids": [2400],
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
        key = name.lower().replace(" ", "").replace("'", "").replace(".", "")
        for cid, c in self.meta()["champs"].items():
            for n in (c.get("alias", ""), c.get("name_en", "")):
                if n.lower().replace(" ", "").replace("'", "").replace(".", "") == key:
                    return int(cid)
        return None


RARITIES = [("kPrismatic", "Prismatic", "#e3b8ff"),
            ("kGold", "Gold", "#f5c518"),
            ("kSilver", "Silver", "#c3cad4")]


def build_view(meta: dict, champ: dict, cid: int, cfg: dict) -> dict:
    augs, lut = meta["augs"], meta.get("itemLut", {})
    info = meta["champs"].get(str(cid), {})

    def rows(entries, n):
        out = []
        for a in entries:
            aug = augs.get(str(a["id"]))
            if aug:
                out.append({"name": aug.get("name_en") or aug.get("name"), "wr": a["wr"],
                            "lift": a.get("lift", 0.0), "games": a.get("g", 0)})
            if len(out) >= n:
                break
        return out

    def item_name(i):
        return lut.get(str(i), {}).get("e") or str(i)

    builds = []
    for g in champ.get("itemClusters", {}).get("groups", [])[:2]:
        core = [item_name(x["id"]) for x in g["core"]]
        nxt = [item_name(x["id"]) for x in g.get("options", [])[:2]]
        builds.append({"name": g.get("name_en", ""), "wr": g.get("wr"),
                       "core": core, "next": nxt})

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


# ------------------------------------------------------------------- worker

class Worker(threading.Thread):
    """Polls the client/game and pushes events to the UI thread via a queue."""

    def __init__(self, cfg: dict, data: Data, events: queue.Queue):
        super().__init__(daemon=True)
        self.cfg, self.data, self.events = cfg, data, events
        self.lcu = LCU(cfg["league_dir"])
        self.reset()

    def reset(self):
        self.phase = None
        self.cid = None
        self.level = None
        self.mayhem = None
        self.game_seen = False

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
            if phase in (None, "None", "Lobby", "Matchmaking", "ReadyCheck", "EndOfGame"):
                keep_cid = phase == "EndOfGame" and self.cid
                self.reset()
                self.emit("hide")
                if keep_cid:
                    self.cid = keep_cid
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
        elif phase == "InProgress" or (phase is None and live("activeplayername")):
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
        view = build_view(meta, champ, cid, self.cfg)
        self.emit("view", view)
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
        if self.cid is None:  # started mid-game: identify our champion from the live API
            name = live("activeplayername")
            players = live("playerlist") or []
            mine = next((p for p in players if name in (p.get("riotId"), p.get("summonerName"))), None)
            raw = (mine or {}).get("rawChampionName", "").rsplit("_", 1)[-1]
            cid = self.data.find_champ(raw) if raw else None
            if cid is None:
                return
            self.cid = cid
            self.load(cid, show=False)
        level = (live("activeplayer") or {}).get("level")
        if not level:
            return
        if not self.game_seen:  # game just loaded: the first augment is offered right away
            self.game_seen = True
            self.emit("flash", self.cfg["auto_show_seconds"])
        elif level != self.level and level in self.cfg["augment_levels"]:
            self.emit("flash", self.cfg["auto_show_seconds"])
        self.level = level


# ----------------------------------------------------------------- Windows UI

def win32():
    import ctypes
    from ctypes import wintypes
    return ctypes, wintypes, ctypes.windll.user32


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


def wr_color(lift: float) -> str:
    if lift >= 0.03:
        return "#4ade80"
    if lift >= 0.01:
        return "#b5e655"
    if lift > -0.01:
        return FG
    return "#f87171"


class Overlay:
    def __init__(self, cfg: dict, events: queue.Queue):
        import tkinter as tk
        self.tk, self.cfg, self.events = tk, cfg, events
        self.root = root = tk.Tk()
        root.title("ARAM Mayhem Overlay")
        root.overrideredirect(True)
        root.configure(bg=BG)
        root.attributes("-topmost", True)
        root.attributes("-alpha", cfg["opacity"])
        self.f = cfg["font_size"]
        self.body = tk.Frame(root, bg=BG, padx=12, pady=8,
                             highlightthickness=1, highlightbackground="#2a3340")
        self.body.pack()
        self.status = "Waiting for champ select…"
        self.view = None
        self.visible = False
        self.hide_job = None
        self.render()
        root.update()
        ctypes, _, user32 = win32()
        self.user32 = user32
        self.hwnd = user32.GetParent(root.winfo_id()) or root.winfo_id()
        # layered + click-through + no taskbar button + never takes focus
        style = user32.GetWindowLongW(self.hwnd, -20)
        user32.SetWindowLongW(self.hwnd, -20, style | 0x80000 | 0x20 | 0x80 | 0x08000000)
        root.attributes("-alpha", cfg["opacity"])
        self._hide()
        root.after(100, self.pump)
        root.after(2000, self.keep_on_top)

    # Show/hide with raw Win32 calls so the overlay never steals focus from the game.
    def _show(self):
        self.user32.ShowWindow(self.hwnd, 4)  # SW_SHOWNOACTIVATE
        self._topmost()
        self.visible = True

    def _hide(self):
        self.user32.ShowWindow(self.hwnd, 0)
        self.visible = False

    def _topmost(self):
        # HWND_TOPMOST, SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE
        self.user32.SetWindowPos(self.hwnd, -1, 0, 0, 0, 0, 0x1 | 0x2 | 0x10)

    def keep_on_top(self):
        if self.visible:
            self._topmost()
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
                elif kind == "show":
                    self._cancel_hide()
                    self._show()
                elif kind == "flash":
                    self._cancel_hide()
                    self._show()
                    self.hide_job = self.root.after(int(args[0] * 1000), self._hide)
                elif kind == "hide":
                    self._cancel_hide()
                    self._hide()
                elif kind == "hotkey" and args[0] == "toggle":
                    self._cancel_hide()
                    self._hide() if self.visible else self._show()
                elif kind == "hotkey" and args[0] == "quit":
                    self.root.destroy()
                    return
        except queue.Empty:
            pass
        self.root.after(100, self.pump)

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
                       row=0, column=0, columnspan=2, sticky="w")
            for r, a in enumerate(col["top"], 1):
                self.label(box, a["name"][:24], row=r, column=0, sticky="w")
                self.label(box, f"{a['wr'] * 100:.1f}%", wr_color(a["lift"]), -1,
                           row=r, column=1, sticky="e", padx=(8, 0))
            if col["avoid"]:
                avoid = "\n".join("✗ " + a["name"][:24] for a in col["avoid"])
                self.label(box, avoid, "#b06a6a", -2, row=len(col["top"]) + 1, column=0,
                           columnspan=2, sticky="w", pady=(4, 0))

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
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s",
                        handlers=[logging.FileHandler(DATA_DIR / "overlay.log", encoding="utf-8"),
                                  logging.StreamHandler()])
    cfg = load_config()
    data = Data(cfg["data_url"], cfg["cache_hours"])
    args = sys.argv[1:]

    if args[:1] == ["--print"]:
        cid = data.find_champ(" ".join(args[1:]) or "Annie")
        view = build_view(data.meta(), data.champ(cid), cid, cfg)
        print(json.dumps(view, indent=1, ensure_ascii=False))
        return

    if sys.platform != "win32":
        sys.exit("The overlay runs on Windows (where League runs). Use --print to test data here.")
    if not single_instance():
        sys.exit("Already running.")
    try:
        import ctypes
        ctypes.windll.shcore.SetProcessDpiAwareness(1)  # crisp text on scaled displays
    except (AttributeError, OSError):
        pass

    events: queue.Queue = queue.Queue()
    overlay = Overlay(cfg, events)
    hotkey_thread([(cfg["toggle_hotkey"], "toggle"), (cfg["quit_hotkey"], "quit")], events)

    if args[:1] == ["--demo"]:
        cid = data.find_champ(" ".join(args[1:]) or "Annie")
        events.put(("view", build_view(data.meta(), data.champ(cid), cid, cfg)))
        events.put(("status", "demo mode"))
        events.put(("show",))
    else:
        Worker(cfg, data, events).start()
    log.info("overlay running (toggle %s, quit %s)", cfg["toggle_hotkey"], cfg["quit_hotkey"])
    overlay.root.mainloop()


if __name__ == "__main__":
    main()
