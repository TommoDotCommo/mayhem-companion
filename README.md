# ARAM Mayhem Overlay

A small always-on-top panel for ARAM: Mayhem. It shows the best and worst augments for your champion in each rarity, then imports that champion's build into the client.

- **Champ select:** the overlay appears when you lock or swap a champion. It writes a `[Mayhem] <Champion>` item set (core builds, boots and high-win-rate items) and sets your recommended summoner spells, keeping Flash on whichever key you already use.
- **In game:** when the augment cards come up, each one gets a tier badge (**S/A/B/C/D**) with your champion's win rate on it, and the best option gets a ★. Rerolls update the badges straight away. Press **F8** to show or hide the full panel. The overlay is click-through and never takes focus from the game.
- **Quit:** Ctrl+Shift+F8.

It needs Windows Python 3.9+. The card badges also use Windows' built-in OCR engine, which needs one install:

    py -m pip install -r requirements.txt

Without that install, everything else still works, and the panel pops up at augment levels instead.

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
| `tier_cutoffs`, `smoothing_games` | how win rate maps to S–D (see below) |
| `scan_interval_fast`, `scan_interval_slow` | seconds between card scans while dead or at game start, and while alive (0 = off) |
| `hide_from_capture` | `true` hides the overlay from OBS and screenshots |
| `ocr_language` | must match your client language (default `en-US`) |
| `import_item_set`, `set_summoner_spells` | turn off the client import |
| `only_mayhem` | `false` to also show the overlay in other queues |
| `per_rarity`, `avoid_per_rarity`, `font_size`, `opacity` | how it looks |
| `league_dir` | change it if League isn't installed in `C:\Riot Games\League of Legends` |

## How it works

- `lockfile` → League client API: game phase, queue, champ select, item sets and spells.
- `127.0.0.1:2999` → Live Client Data API: game time and whether you're dead. Augments can only be picked at game start or while dead, so the card scan runs quickly then and slowly otherwise.
- **Card reading:** the screen is captured in thin horizontal strips with GDI, read with `Windows.Media.Ocr`, and the text is matched against the augment list, allowing for small misreads. Whole-card captures confuse the OCR, which reads the card frames as giant letters. Once the titles are found, that strip is saved to `layout.json` for your resolution, so later scans cost about 10 ms. Delete that file if badges ever stop appearing.
- Stats come from [arammeta.com](https://arammeta.com) ([source, MIT](https://github.com/Lanternko/ARAM-Mayhem-Database)), built from Mayhem (queue 2400) match data. They are cached in `%LOCALAPPDATA%\AramOverlay\cache` for 12 hours, so each champion costs about one 90 KB request per day.
- Log file: `%LOCALAPPDATA%\AramOverlay\overlay.log`.

**Tiers** compare each augment's win rate on your champion with that champion's overall win rate. Small samples are first pulled toward the average (`smoothing_games`), so 40 lucky games can't make an augment S tier. Default cut-offs: S ≥ +2.0%, A ≥ +0.8%, B ≥ −1.0%, C ≥ −2.5%, and D below that. An augment with no data for your champion uses its all-champion win rate and is marked "(all)".

**Limits:** the card text must be in English, matching `ocr_language`. An augment missing from the current dataset, such as one that was just added, gets no badge.
