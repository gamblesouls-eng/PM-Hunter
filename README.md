# Poopihunt

Hunts wild encounters in the Roblox Pokemon game: runs from the rest, catches any
Pokemon with a reskin and a gradient, and pings Discord. Nothing to configure:
paste your Discord webhook into it, and that's all. It fits itself to any window
or phone screen.

## Using it

Download it from **https://poopihunt.xyz**.

- **Windows**: run `PoopihuntSetup.exe` ("Windows protected your PC": **More
  info > Run anyway**).
- **Mac**: unzip `Hunt-mac.zip` into Applications; the first time, System
  Settings > Privacy & Security > **Open Anyway**, then allow Screen Recording
  and Accessibility.
- **Android** (11 or newer): install the APK, open Poopihunt, and turn on what
  it shows (Accessibility; Screen capture on some phones). Then open Roblox.

**The pill** floats over the game:

- **Play / stop.** A right click (computer) or long press (phone) is a
  **debug run**: it hunts, and at the next encounter checks every step (a Quick
  Ball, Yawn, running, the bag, saving) and reports to the logs. A new
  install's first hunt does one by itself.
- **The website code** (tap to copy on a phone). Type it on
  https://poopihunt.xyz/app to watch, take over a battle and throw balls from
  anywhere. A long press makes a new code.
- **The webhook box** (computers; on a phone it's in the app).

A find turns the pill white and holds the battle for you. Ctrl+Shift+H pauses
and resumes on a computer. Computers keep themselves up to date (a hunt carries
on through it); phones update when you press stop.

## Working on it

All Rust, one Cargo workspace:

- `core/`: the hunter, the same everywhere: reading the screen, fitting the
  layout, the rules and catch plan (built in: `core/src/config.rs`), the bag and
  MENU, saving, the website link, Discord and the logs.
- `window/`: Hunt / Hunt.exe, the pill with the hunter inside, and each
  desktop's screenshots, clicks and text recognition (`window/src/desktop/`).
- `android/`: the app (Kotlin) around `android/rust/` (hunt-core through UniFFI).
- `server/` and `site/`: poopihunt.xyz, the API and the page.

**Test**: `cargo test -p hunt-core` (checks the readings against `samples/`,
and runs a hunt on a pretend screen); `cd android && ./gradlew testDebugUnitTest`.

**Run locally**:

    HUNT_DATA=$PWD cargo run --release -p hunt              # the pill (HUNT_WATCH=1: never clicks)
    cargo run --release -p hunt -- --terminal               # the hunter alone (--watch, --debug, --paused)
    cargo run --release -p hunt -- --diagnose               # read the samples, check the screen
    cargo run --release -p hunt -- --image shot.png         # read one screenshot

On a Mac, `Poopihunt.command` builds and opens the pill from Terminal.

**Publish** (each puts its build on poopihunt.xyz; push first for Windows):

- `./publish-windows.sh`: built on GitHub (`.github/workflows/windows.yml`,
  Inno Setup with `windows/hunt.iss`), which also reads the samples with
  Windows' text recognition and checks the pill comes up.
- `./publish-mac.sh`: `make-mac-app.sh`, signed with this Mac's certificate.
- `./publish-android.sh`: needs the Android SDK, NDK 27,
  `rustup target add aarch64-linux-android` and `cargo install cargo-ndk`.
- `./publish-server.sh`: the API and the page (`--setup` the first time).

**Icons**: `python windows/make_icon.py` makes every one from
`windows/hunt-icon.png`.

**Logs**: every Hunt sends its log to poopihunt.xyz (`core/src/out.rs`): each
line, each encounter's reads with what the box's text was, what went wrong with
its picture, crashes and updates; kept 7 days. Read them on
https://poopihunt.xyz/app/logs/ in a browser logged in (on /app) to one of the
devices in the server's `data/admins.json` (`["device id", ...]`). The pill's
**Send problem reports** off sends nothing.

**Server**: poopihunt.xyz is in `/srv/poopihunt.xyz` on the stingray server
(owner `poopihunt`, nginx site `poopihunt.xyz`, the `poopihunt-api` service),
kept apart from stingray-digital.online.
