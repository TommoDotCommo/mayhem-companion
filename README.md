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
  - Your builds are imported as `[Mayhem]` item sets, one page per build style.
  - Your summoner spells are set, keeping Flash on the key you already use.
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
| **Tab** (held) | syncs the relic timers from the scoreboard |
| **Alt+1–4** | relic taken: blue inner, blue outer, red outer, red inner |
| Tray icon | left-click opens the companion; right-click offers *Start with Windows* and *Quit* |

Settings live in `%LOCALAPPDATA%\AramOverlay\config.json` (created on first run). The log is `overlay.log` in the same folder.

## Privacy and fair play

- Everything runs on your PC. It reads the League client's local API (champ select, item sets), the game's local Live Client Data API (your level, items, game clock), and small parts of your screen: card titles, the minimap's relic area, and the scoreboard's relic timers while Tab is held. Screen captures are processed in memory with Windows' built-in text recognition and never saved or sent anywhere.
- The only network requests download public statistics from arammeta.com, Data Dragon and the League of Legends Wiki.
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
   Opens for champ select and closes when the game starts, so its GPU memory
   is only paid while it's useful.
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

Tests: `cargo test`. Releases: push a version tag (`git tag v0.3.1 && git push origin v0.3.1`) and GitHub builds and publishes the exe. Debug tools: `--demo <Champion> [round]` (fake champ select), `--scan <Champion>` (one screen sweep, logged), `--render <in.png> <out.png> <Champion>` (draws the overlay onto a screenshot), and `--balance` (logs balance data). Set the `ARAM_DEBUG` environment variable for debug logging, plus pictures of what the relic watch saw.

## Licence

[MIT](LICENSE).
