# Mayhem Companion

An unofficial helper for League of Legends **ARAM: Mayhem**. It's a native Windows app written in Rust: a single ~7 MB `.exe` with no installer and no runtime to install.

> Mayhem Companion isn't endorsed by Riot Games and doesn't reflect the views or opinions of Riot Games or anyone officially involved in producing or managing Riot Games properties. Riot Games, and all associated properties are trademarks or registered trademarks of Riot Games, Inc.

## What it does

- **Augment badges in game.** When augment cards come up, each card gets a badge just above it:
  - **Tier and win rate** (**S+ S A B C D F**) for your champion, at the pick round you're on (R1–R4).
  - **Rank** among every augment of that rarity for your champion and round, e.g. **3rd / 58**. The top 10% are green and the bottom 10% red. The single best augment gets a green frame and the single worst a red one.
  - **Build fit** with the augment's category: **✓ FITS BUILD · AP**, **✗ OFF-BUILD · Crit** or **• ANY BUILD · Economy**.
  - Rerolls update the badges straight away. **F8** toggles a panel with your champion's best augments and builds.
- **Heal relic timers on the minimap.** A countdown sits on each relic that's down; relics that are up keep the game's own "+" icon.
  - First spawns come from the game clock (1:45 outer, 2:30 inner).
  - Pickups are detected from the minimap. The "+" disappears when the heal lands, which is when the 90 s respawn starts. When a champion icon hides the relic, the timer is worked out from when the icon reached it, and the delay is learned from your own games.
  - Holding **Tab** syncs all four timers from the game's own scoreboard.
  - Timers turn yellow for their last 10 seconds. **Alt+1–4** marks a relic taken by hand (press again within 3 s to undo).
- **Champ select.** The companion window opens with your pick, bench and team, each with a tier and win rate. The bench shows the win-rate gap against your pick; you swap with one click, and you can reroll from there.
  - Your builds are imported as `[Mayhem]` item sets and placed first. Each page opens with a Start row (what your starting gold buys towards the first item, plus a Refillable Potion if 150g is spare) and then walks through the build as numbered buy steps: an item's components (dearest first), then the item, with boots second. Choose one page per build style or all builds on one page in **Settings**. Recognised tool builds for your current champion (Blitz, Porofessor, Mobalytics, U.GG, OP.GG and others) can be replaced, including the loose build files some tools leave in the game's `Config` folder (those open ahead of every item set, for every champion); personal builds and builds for other champions or maps are kept.
  - Your summoner spells are set, keeping Flash on the key you already use.
- **Team-aware bench recommendations.** Bench picks are ordered by champion strength, reliable teammate pairings, role gaps and damage balance. Hover a pick to see the score and reasons; fit points are a heuristic, not predicted win probability. Champion win rates remain visible separately.
- **Personal match history.** The **History** tab imports your recent Mayhem matches from the running League client and keeps them locally, with results, KDA, champion damage, final items and augments. Filter by champion or patch and compare your own champion, item and augment results. Accounts are stored separately using an opaque account key; no other players' identities or statistics are saved. The app checks the latest 40 matches on connection and about once a minute while outside a game or champion select, and retries incomplete results. Saved history accumulates across sessions and remains available offline. **Highlights** pick out what tends to go well for you: how you do against everyone's win rate on the same champions, your strongest and weakest champion classes, build styles and augment types, standout augments, game length, deaths and streaks. A highlight appears only once a pattern has enough games behind it, and small records are pulled toward your average. Small personal samples describe your results and do not establish an item's or augment's independent effect.
- **Challenge tracker.** The **Challenges** tab has three sections. **ARAM God** shows your progress toward the title (Master in ARAM Authority), every ARAM title and how far each one is, and the challenges closest to their next level. **ARAM challenges** lists each ARAM Warrior, Finesse and Champion challenge with a progress bar. **Any mode** lists challenges that aren't ARAM-specific but that Mayhem games still move: champion mastery and summoner level. One-per-champion challenges such as All Random All Champions open into a checklist of the champions left and done, with a search box. In champion select, your pick and each bench champion carry a row of dots, one per one-per-champion challenge that champion counts toward (filled when already done on it); hover the champion for the list of what is done and still to do. Turn this off in **Settings**. Mayhem games count toward the ARAM challenges. Progress is read from the League client after each game and about every 10 minutes, and saved locally so it stays visible offline.
- **Patch-change summaries.** The **Patch Changes** tab compares champions, augments and items with the previous patch, showing rising and falling win rates, percentage-point changes, and sample sizes. These are observed statistical trends, rather than a list of balance edits.
- **Champion pages.**
  - **Balance changes:** ARAM and Mayhem balance changes appear as green (buff) and red (nerf) chips, combined the way the game applies them, with Mayhem's ability changes on hover.
  - **Builds:** one card per build style. Each win rate is coloured against the champion's average, with tags for the highest win rate and the most played.
  - **Augments:** the best four per rarity are shown, and every augment is one click away.
- **Augment tier list** showing which champions each augment is best and worst on.
- **Remembers its place.** The companion reopens on the same monitor, position and size as last time.

## Install

1. Download `mayhem-companion.exe` from the [latest release](https://github.com/TommoDotCommo/mayhem-companion/releases/latest).
2. Put it in a folder of its own and run it. It starts a small background service (tray icon) and opens the companion window.
3. Set League to **Borderless** or **Windowed**. Nothing can draw over exclusive Fullscreen.

**Windows Defender / SmartScreen.** The exe isn't code-signed yet. Defender's machine-learning scanner has flagged earlier builds as `Trojan:Win32/Bearfoos.A!ml`. That's a false positive that unsigned apps capturing the screen and registering hotkeys often get. Each release lists the file's SHA-256 so you can check your download. You can also build it yourself from this source (see below). If Defender quarantines it, you decide whether to allow it; reports go to [Microsoft's false-positive form](https://www.microsoft.com/en-us/wdsi/filesubmission).

## Hotkeys

| | |
| --- | --- |
| **F7** | show or hide the companion (starts it if it's closed) |
| **F8** | in-game panel |
| **Ctrl+Shift+F8** | quit everything |
| **F9** | in game: save a diagnostic screenshot of this moment (see Settings) |
| **Tab** (held) | syncs the relic timers from the scoreboard |
| **Alt+1–4** | relic taken: blue inner, blue outer, red outer, red inner |
| Tray icon | left-click opens the companion; right-click offers *Start with Windows* and *Quit* |

Open the **Settings** tab to keep the companion open while playing, choose how item builds are grouped, and control automatic updates. Changes save immediately and apply to the running service; changing build settings in champion select reimports your builds.

App updates are checked on startup and every six hours by default. The app downloads the latest stable GitHub release and verifies its SHA-256 digest. To install a downloaded update, **Quit** from the tray and open the app again. The previous executable is kept as `.previous`; installation errors are recorded in `%LOCALAPPDATA%\AramOverlay\updates\install-error.txt`. Updates require a writable app folder. Use **Check for app updates** for a manual check.

ARAM data refreshes automatically every 12 hours while the app stays open, including new data published during the same patch. **Refresh ARAM data now** forces a refresh. Both update options can be disabled independently, and cached data stays available when offline.

Settings live in `%LOCALAPPDATA%\AramOverlay\config.json` (created on first run). The log is `overlay.log` in the same folder. Personal match history lives in the `history` subfolder.

## Privacy and fair play

- Everything runs on your PC. It reads the League client's local API (champ select, item sets, your match history and challenge progress), the game's local Live Client Data API (your level, items, game clock), and small parts of your screen: card titles, the minimap's relic area, and the scoreboard's relic timers while Tab is held. Screen captures are processed in memory with Windows' built-in text recognition and never sent anywhere. They are only saved if you press **F9** in a game or turn on **Save a screenshot whenever augment badges appear** in Settings (off by default): that keeps full-screen screenshots, chat and player names included, in the `diagnostics` folder on your PC so you can zip them up for a bug report yourself.
- Personal results are stored locally and never uploaded. Account identifiers are hashed before storage, and names and other players' results are discarded.
- Network requests download public statistics from arammeta.com, Data Dragon and the League of Legends Wiki, and app release metadata and updates from GitHub.
- It doesn't read game memory, automate input, or show anything about enemies that the game itself doesn't show you. Relic timers are neutral objectives the game already displays on the scoreboard.

## Data and credits

- Augment, champion and build statistics: [arammeta.com](https://arammeta.com) ([source, MIT](https://github.com/Lanternko/ARAM-Mayhem-Database)), built from about 800k Mayhem games per patch.
- Item data and images: Riot's [Data Dragon](https://developer.riotgames.com/docs/lol#data-dragon).
- ARAM and Mayhem balance changes: the [League of Legends Wiki](https://wiki.leagueoflegends.com/en-us/ARAM:_Mayhem) (`Module:ChampionData` and the ARAM: Mayhem page), licensed [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/).

## How the tiers work

Each augment's win rate on your champion is compared with that champion's average. Small samples are pulled toward the average (`smoothing_games`, 300), and each pick round's sample is pulled toward the augment's overall result. The shown win rate is that adjusted number, and the rank orders augments by it.

Cut-offs: S+ ≥ +3.5%, S ≥ +2%, A ≥ +0.8%, B ≥ −1%, C ≥ −2.5%, D ≥ −4%, and F below that. Champions use the same idea against 50%.

**Build fit** scores your items by their actual stats (Data Dragon). Before you've bought anything it uses your champion's most common build. It's a rule of thumb, not a win rate: nobody publishes augment win rates split by build.

## Architecture

```
mayhem-companion.exe  (service, ~45 MB, always on)
├─ lcu       League client: lockfile auth + WAMP websocket events (no polling)
├─ engine    champ select → item sets + spells; in game: live API once a second
├─ scan      DXGI desktop duplication → Windows.Media.Ocr → fuzzy name match
│            (newest frame stays on the GPU; only the needed strip is copied back;
│             unchanged desktop or unchanged strip = no copy / no OCR; GDI fallback)
├─ relics    minimap watch (10 Hz, ~250 px square) + scoreboard sync while Tab is held
├─ overlay   layered Win32 windows drawn with Direct2D/DirectWrite;
│            click-through, never focused, hidden from capture, no render loop
└─ ipc       127.0.0.1 JSON lines → companion

mayhem-companion.exe --companion  (egui, GPU-rendered; repaints only on change)
   Opens for champ select; optionally stays open during games (Settings).
```

**Measured** (2560×1440):

| | |
| --- | --- |
| First sweep to find the cards | 145 ms, once per resolution |
| Strip scan with OCR | 8 ms |
| Strip copy + fingerprint (screen changed, no OCR) | 0.16 ms (GDI: 7 ms) |
| Desktop unchanged | 0.004 ms |
| Service RAM | ~45 MB idle, ~65 MB in game |
| Companion RAM | ~100 MB while open; it closes when the game starts |

## Build from source

On Windows, with [Rust](https://rustup.rs):

    cargo build --release
    # target\release\mayhem-companion.exe

From WSL or Linux, with `rustup target add x86_64-pc-windows-gnu` and mingw-w64 installed:

    cargo build --release --target x86_64-pc-windows-gnu

Tests: `cargo test` on Windows, or `cargo test --lib` for portable rating and item-set tests on Linux. Releases are tested, built and published locally; GitHub Actions is disabled. On Windows, run `cargo test --locked` and `cargo build --release --locked`. From WSL, compile the Windows test executables with `cargo test --target x86_64-pc-windows-gnu --no-run --locked`, run both test executables on Windows, then build with `cargo build --release --target x86_64-pc-windows-gnu --locked`. Bump the version in Cargo.toml, Cargo.lock and the Windows resources, prepare notes under `docs/releases/`, commit, push, and tag the release. Create the GitHub release as a draft, upload the local executable and its SHA-256 file, verify the downloads, then publish the draft. Debug tools: `--demo <Champion> [round]` (fake champ select), `--scan <Champion>` (one screen sweep, logged), `--render <in.png> <out.png> <Champion>` (draws the overlay onto a screenshot), and `--balance` (logs balance data). Set the `ARAM_DEBUG` environment variable for debug logging, plus pictures of what the relic watch saw.

## Licence

[MIT](LICENSE).
