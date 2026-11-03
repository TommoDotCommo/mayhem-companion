# ARAM Mayhem

A native Windows app for ARAM: Mayhem, written in Rust. It's a single 7 MB `.exe` that needs no runtime.

- **In game:** when augment cards come up, each one gets a badge just above the card, all the same size:
  - **Tier and win rate** (**S+ S A B C D F**) for your champion, at the pick round you're on (R1–R4).
  - **Rank** among every augment of that rarity for your champion and round, e.g. **3rd / 58**. The top 10% are green and the bottom 10% red; the single best augment gets a green frame and the single worst a red one.
  - **Build fit** with the augment's category: **✓ FITS BUILD · AP**, **✗ OFF-BUILD · Crit** or **• ANY BUILD · Economy**.
  - Rerolls update the badges straight away. **F8** toggles a panel with your champion's best augments and builds.
- **Heal relic timers:** on the minimap, a countdown sits on each relic that's down; relics that are up keep the game's own "+" icon. The minimap is found automatically from its frame (either corner, any size), or set `relic_minimap` to `[x, y, size]`. Until it's found, or with `relic_on_minimap` off, a bar at the top of the screen shows all four.
  - **First spawns:** automatic from the game clock (1:45 outer, 2:30 inner).
  - **Pickups are automatic:** the app watches each relic's "+" on the minimap. When it disappears the 90 s respawn starts; when it reappears early the timer is corrected. A champion icon covering a relic doesn't count as a pickup.
  - **Respawns:** holding **Tab** syncs all four from the game's own scoreboard timers; they keep counting after you let go.
  - **Manual backup:** **Alt+1–4** marks a relic taken (press again within 3 s to undo).
  - Timers turn yellow for their last 10 seconds.
- **Champ select:** the companion window opens with your pick, bench and team, each with a tier and win rate. The bench shows the win-rate gap against your pick, and you swap with one click; you can also reroll from there. Your builds are imported as `[Mayhem]` item sets, one page per build style (e.g. *AP burst*, *AP bruiser*), with core, full build, next items, variants and boots, and your summoner spells are set, keeping Flash on the key you already use.
- **Browse:** a champion tier list, with each champion's augments (by rarity and pick round) and core builds that import with one click. There's also an augment tier list showing which champions each augment is best and worst on.

Data comes from [arammeta.com](https://arammeta.com) ([MIT source](https://github.com/Lanternko/ARAM-Mayhem-Database)), built from about 800k Mayhem games per patch. Not endorsed by Riot Games.

## Use

Run `aram-mayhem.exe`. It starts a small background service (tray icon) and opens the companion window.

| | |
| --- | --- |
| **F7** | show or hide the companion (starts it if it's closed) |
| **F8** | in-game panel |
| **Ctrl+Shift+F8** | quit everything |
| **Tab** (held) | syncs the relic timers from the scoreboard |
| **Alt+1–4** | relic taken: blue inner, blue outer, red outer, red inner |
| Tray icon | left-click opens the companion; right-click offers *Start with Windows* and *Quit* |

**League must be in Borderless or Windowed mode.** Nothing can draw over exclusive Fullscreen.

Settings live in `%LOCALAPPDATA%\AramOverlay\config.json` (created on first run), and the log is `overlay.log` in the same folder.

## Architecture

```
aram-mayhem.exe  (service, ~40 MB, always on)
├─ lcu       League client: lockfile auth + WAMP websocket events (no polling)
├─ engine    champ select → item set + spells; in game: live API once a second
├─ scan      DXGI desktop duplication → Windows.Media.Ocr → fuzzy name match
│            (newest frame stays on the GPU; only the title strip is copied back;
│             unchanged desktop or unchanged strip = no copy / no OCR; GDI fallback)
├─ relics    game-clock relic timers; scoreboard OCR sync while Tab is held
├─ overlay   layered Win32 windows drawn with Direct2D/DirectWrite;
│            click-through, never focused, hidden from capture, no render loop
└─ ipc       127.0.0.1 JSON lines → companion

aram-mayhem.exe --companion  (egui, GPU-rendered; repaints only on change)
   Opens for champ select and closes when the game starts, so its GPU memory
   (~60–200 MB, mostly the graphics driver) is only paid while it's useful.
```

The OCR engine is created when a Mayhem game starts and released a minute after it ends. Card titles are read in horizontal strips, because whole-card captures make the OCR read the card frames as letters. The strip is remembered per resolution in `layout.json`.

**Measured** (2560×1440, demo cards on screen):

| | |
| --- | --- |
| First sweep to find the cards | 145 ms, once per resolution |
| Strip scan with OCR | 8 ms |
| Strip copy + fingerprint (screen changed, no OCR) | 0.16 ms (GDI: 7 ms) |
| Desktop unchanged | 0.004 ms |
| Service RAM | ~40 MB idle, ~60 MB scanning in game |
| Companion RAM | ~100 MB while open; it closes when the game starts |

## Tiers

Each augment's win rate on your champion is compared with that champion's average. Small samples are pulled toward the average (`smoothing_games`, 300), and each pick round's sample is pulled toward the augment's overall result. The shown win rate is that adjusted number, and the rank orders augments by it.

Cut-offs: S+ ≥ +3.5%, S ≥ +2%, A ≥ +0.8%, B ≥ −1%, C ≥ −2.5%, D ≥ −4%, and F below that. Champions use the same idea against 50%.

**Build fit** scores your items by their actual stats (Data Dragon); before you've bought anything it uses your champion's most common build. It's a rule of thumb, not a win rate: nobody publishes augment win rates split by build.

## Build

From WSL or Linux, with `rustup target add x86_64-pc-windows-gnu` and mingw-w64 installed:

    cargo build --release --target x86_64-pc-windows-gnu
    # target/x86_64-pc-windows-gnu/release/aram-mayhem.exe

Test tools: `--demo <Champion> [round]` (fake champ select; scans whatever cards are on screen) and `--scan <Champion>` (one sweep, logged).

## Windows Defender

Defender's ML scanner has flagged this unsigned build as `Trojan:Win32/Bearfoos.A!ml`, a false positive. That's typical for fresh unsigned executables that capture the screen and register global hotkeys; Defender flagged Blitz too. The real fixes are code signing or [reporting the false positive to Microsoft](https://www.microsoft.com/en-us/wdsi/filesubmission). Allowing it on your own machine is your call.
