<p align="center">
  <img src="assets/icon/app-256.png" width="88" alt="SMB shuriken mark">
</p>

<h1 align="center">SMB // Companion</h1>

<p align="center">
  <b>SMB — Sexy Maar Bescheiden.</b> Dutch for “Sexy, but modest.”<br>
  A small companion for SMB players. League setup and a Minecraft pack that stays ready before play.
</p>

<p align="center">
  <img src="docs/setup-applied.png" width="360" alt="Setup applied in champion select">
  &nbsp;
  <img src="docs/standing-by.png" width="360" alt="Connected and standing by">
</p>

## What it does

- **Runes.** Applies a recommended rune page for your champion and role the moment you pick.
  It writes to one page of its own (`SMB // Lee Sin · Jungle`). Your own pages are never edited or deleted.
- **Summoner spells.** Sets the recommended pair with Flash on your key. It learns that key from your own
  games: where you left Flash last time, or where you move it in champion select. Nothing to set up.
- **Shows its work.** Champion, role, keystone, rune trees and the spells on D/F, in League's own art,
  loaded from your local client. Hover for names in your client's language.
- **Item set & skill order** *(optional, off by default)*. OP.GG's most played build as an item set in
  your in-game shop, with the skill order in its first row. One set of its own; your other item sets stay as they are.
- **Modes.** Ranked, normals, ARAM, ARAM Mayhem, URF, Arena and Swiftplay. Sits TFT out.

## Minecraft (preview)

Choose **Minecraft**, select the SMB instance you already imported into **Modrinth App 0.23.1**, then use **Play SMB**.
SMB checks the stable client channel, verifies the exact pack and mod checksums, and updates that instance before handing launch to Modrinth.
Automatic updates, launch after update, and checking direct Modrinth launches have separate toggles. Checking a compatible older pack does not force a server-only release update; **Repair pack** explicitly applies the latest compatible runtime.

For direct launches, copy the command shown in the app into the instance's **Custom game launch hooks → Pre-launch** field, save, then refresh SMB to verify it. Keep existing custom hooks unchanged. The helper waits for the update, returns a failure code when launch must stop, and never opens or focuses the companion window.

- Updates cover pack-owned mods and defaults. Personal config edits, added mods, worlds and screenshots stay intact. Modified pack-owned mods cause a conflict; missing mods require explicit repair.
- Minecraft/Fabric runtime changes need a native Modrinth runtime change first. SMB never edits the launcher database or reads account credentials.
- Any running Java/Minecraft process defers updates. Interrupted changes are rolled back under an OS lock before another update; a pending recovery blocks launch even when checks are disabled.
- Native resource-pack synchronization must be off for the selected instance. Native synced options/servers are excluded when first adopting it; enabling overlapping native sync later requires turning it off before further updates.
- An unavailable channel blocks checked launches. Disable automatic checks explicitly to launch an intact existing pack without checking; interrupted installations still block.

The public client feed is **not published yet**. The preview defaults to `jmpijll/smb-minecraft-client` and will honestly report it unavailable until those reviewed client artifacts are published. Developers can set `SMB_MINECRAFT_CHANNEL` to an explicit local `stable.json` for disposable-profile testing. No private GitHub token is needed or bundled. Client feed export instructions are in [the implementation notes](docs/minecraft/IMPLEMENTATION.md).

The installed executable, updater repository and settings location remain compatible with existing `smb-lol` installations. Minecraft preferences are separate from League preferences.

## Stays out of your game

The game has the focus. The companion doesn't ask for it.

- It never opens, raises or focuses a window by itself. It only appears when you click the tray icon or launch it again.
- Launched while a game is already running? It starts in the tray.
- **Start with Windows** goes straight to the tray, silently.
- No overlays, no notifications, no taskbar flashing, no always-on-top.
- Runs at below-normal CPU priority and polls less while a game is in progress.
- One GUI instance at a time. League uses the client's local API (LCU) and only writes during champion select.
  Minecraft updates explicitly owned instance files before play. Neither feature opens a game process or reads game memory.

## Install

Grab one from [Releases](../../releases/latest):

| | **Installer** `smb-companion-<version>-setup.exe` | **Portable** `smb-companion-<version>-portable.zip` |
| --- | --- | --- |
| Admin rights | Not needed (installs for your user only) | Not needed |
| Where | `%LOCALAPPDATA%\Programs\SMB Companion` | Any folder you unzip it to |
| Settings | `%APPDATA%\SMB\LoL Companion` | Inside that folder |
| Extras | Start menu, optional desktop shortcut and *Start with Windows*, uninstaller | — |
| Updating | Automatic: installs new versions while League, Minecraft/Java and pack updates are idle | Replace `smb-lol.exe`; the app shows when a new version is out |

Builds are **not code-signed yet**, so Windows SmartScreen will warn you: choose **More info → Run anyway**.
Compare the file against `SHA256SUMS.txt` if you like to be sure.

Close the window and it keeps running in the tray. Right-click the shuriken for
**Open**, **Automatic runes & spells** (on/off) and **Quit**.

**Uninstall.** Installer: *Settings → Apps → SMB Companion*. Your settings file is kept.
Portable: switch off *Start with Windows*, quit from the tray, delete the folder.

Portable mode comes from the `smb-lol.portable` file next to the exe. Delete it and the same exe uses `%APPDATA%` instead.

Coming from the earlier *Quiet Select* build? Quit it first. This app takes over its rune page instead of making a second one.

## Good to know

- Recommendations come from OP.GG's public champion data. SMB and this project are not affiliated with OP.GG.
  No internet, or OP.GG unavailable? League's own recommended runes and spells fill in. Nothing is guessed.
- Rune page slots full? Delete one in the client and the companion takes it from there.
  Nothing of yours gets deleted to make room.
- Problems retry on their own. **Activity log** (under *Connection & troubleshooting*) shows what happened and when.
- The installed version checks its update feed once a day (GitHub Pages, with the latest release as a fallback).
  Only an update signed with the project's release key is installed, so a hijacked GitHub account or download can't push one.
  It installs silently when you're not queueing, in champion select, in a game or looking at the window.
- Third-party tools that use the League client API are common (rune importers, stat sites). Use it at your own discretion.

## Build from source

Requires Rust (stable, MSVC toolchain) on Windows.

```sh
cargo test
cargo test update::tests::live -- --ignored # live signed update feeds and their installer
cargo build --release          # target/release/smb-lol.exe
python -m unittest discover -s tools -p test_export_minecraft_channel.py
python -m unittest discover -s tools -p test_minecraft_cli.py # requires a debug build + installed Modrinth 0.23.1
```

Development flags:

| Flag | Effect |
| --- | --- |
| `--background` | Start in the tray (what *Start with Windows* uses). |
| `--quit` | Close a running copy cleanly (used by the installer). |
| `--demo --screenshot out.png` | Render a sample champion-select state to a PNG and exit. |
| `--minecraft` | Open the Minecraft page on a normal launch. |
| `--demo-minecraft --screenshot out.png` | Render a synthetic Minecraft screen without live settings or actions. |
| `--demo-minecraft --minecraft-demo-state setup` | Show the one-time hook setup example (also accepts `error`). |
| `--minecraft-prelaunch` | Synchronous, headless Modrinth hook; reads `INST_DIR`, exits 0 or 73. |
| `--screenshot out.png --live` | Capture mode; live workers and external actions stay disabled. |

The shuriken icons and installer artwork are generated by `python tools/make_icons.py` (Pillow).

## Packaging and releasing

`packaging/package.ps1` turns a release build into the installer (Inno Setup 6.7+, `packaging/smb-lol.iss`),
the portable zip and `SHA256SUMS.txt` in `dist/`. CI does this on every push and keeps the result as a build artifact.

```powershell
cargo build --release
./packaging/package.ps1 -Iscc "C:\path\to\Inno Setup 6\ISCC.exe"
```

To publish, bump `version` in `Cargo.toml`, then push a matching tag. GitHub Actions tests, packages and creates the release:

```sh
git tag v0.3.0
git push origin v0.3.0
```

Then ship it to installed copies. On the maintainer's PC, once the release is up:

```powershell
pwsh packaging\sign-update.ps1 -Tag v0.3.0
```

This signs the installer with the release key (`~\.smb-companion-release\update-key.pk8`, never in CI) and
puts the signed `update.json` on GitHub Pages and on the release. Installed copies update silently within a day.
Keep a backup of that key: installed copies trust only it, so without it, members would have to reinstall by hand.

The exe and installer are not Authenticode-signed yet (SmartScreen warns on a first manual install).

## Brand

Styled to the SMB Brand Guide v1.0 (“Neon Shinobi Noir”): Void Black, Blade White and SMB Neon Pink `#F91079` only,
Barlow Condensed and Inter, 12° diagonals, sharp 0–4 px corners, `//` labels, dry copy.
The shuriken here is a provisional geometric rendering of the compact SMB mark until the official vector master exists.

## License and legal

Code: [MIT](LICENSE). Fonts: SIL Open Font License, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
The SMB name, shuriken mark and visual identity belong to SMB — Sexy Maar Bescheiden and are not covered by the MIT license.

SMB Companion isn't endorsed by Riot Games and doesn't reflect the views or opinions of Riot Games or anyone
officially involved in producing or managing Riot Games properties. Riot Games and all associated properties are
trademarks or registered trademarks of Riot Games, Inc. No Riot artwork is bundled; icons are read from your own
installed League client at runtime.

<p align="center"><sub>Play hard. Stay modest.</sub></p>
