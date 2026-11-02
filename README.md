# ARAM Mayhem Overlay

A small always-on-top panel for ARAM: Mayhem. It shows the best and worst augments for your champion in each rarity, then imports that champion's build into the client.

- **Champ select:** the overlay appears when you lock or swap a champion. It writes a `[Mayhem] <Champion>` item set (core builds, boots and high-win-rate items) and sets your recommended summoner spells, keeping Flash on whichever key you already use.
- **In game:** the overlay pops up for 25 seconds when the game loads and at each augment level (3/7/11/15), then hides itself. Press **F8** to show or hide it anytime. It is click-through and never takes focus from the game.
- **Quit:** Ctrl+Shift+F8.

It uses only the Python standard library (Windows Python 3.9+), so there is nothing to install.

## Run

Double-click `run.bat`, or run `pythonw aram_overlay.py`.

- Preview without League: `python aram_overlay.py --demo "Miss Fortune"`
- Data as text: `python aram_overlay.py --print Jinx`

**League must be in Borderless or Windowed mode.** Overlays cannot draw over exclusive Fullscreen. Change it in Settings → Video → Window Mode.

## Settings (`config.json`)

| key | meaning |
| --- | --- |
| `toggle_hotkey`, `quit_hotkey` | e.g. `F8`, `ctrl+shift+a`, `alt+space` |
| `x`, `y` | position in pixels; a negative `x` is the distance from the right edge |
| `augment_levels`, `auto_show_seconds` | when to pop up in game, and for how long |
| `import_item_set`, `set_summoner_spells` | turn off the client import |
| `only_mayhem` | `false` to also show the overlay in other queues |
| `per_rarity`, `avoid_per_rarity`, `font_size`, `opacity` | how it looks |
| `league_dir` | change it if League isn't installed in `C:\Riot Games\League of Legends` |

## How it works

- `lockfile` → League client API: game phase, queue, champ select, item sets and spells.
- `127.0.0.1:2999` → Live Client Data API: your level in game.
- Stats come from [arammeta.com](https://arammeta.com) ([source, MIT](https://github.com/Lanternko/ARAM-Mayhem-Database)), built from Mayhem (queue 2400) match data. They are cached in `%LOCALAPPDATA%\AramOverlay\cache` for 12 hours, so each champion costs about one 90 KB request per day.
- Log file: `%LOCALAPPDATA%\AramOverlay\overlay.log`.

Augments in each column are ranked by arammeta's confidence-weighted score, not raw win rate. Green means a clear win-rate lift for that champion; red means below average.
