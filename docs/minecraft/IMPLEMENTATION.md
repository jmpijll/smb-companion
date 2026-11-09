# Minecraft companion implementation notes

This is a local review build on `feat/minecraft-companion`. The public client feed has not been published. Existing `smb-lol` installation, updater and settings identities stay compatible; the GUI now offers League and Minecraft.

## Player flow

1. Import a stable SMB mrpack in Modrinth App 0.23.1 using its normal import UI.
2. Open the Minecraft page. A single SMB instance is picked automatically; with several, choose one. Use Play. Updates are synchronous before native launch handoff. Switching off launch after update makes this an update-only action.
3. For launches directly from Modrinth, copy the hook and paste it into the instance's native pre-launch hook once. The companion notices it by itself (a quiet look every few seconds while something is missing). Existing custom hooks are never replaced.

Same Minecraft/Fabric runtime only. When the runtime changes, migrate/import the new pack in native Modrinth, then select that instance in SMB; changing only the runtime setting cannot migrate the old pack. Java/Minecraft activity blocks updates conservatively; the helper has no GUI, notifications or focus calls. A checked offline launch blocks with a useful error; automatic checks can explicitly be disabled for an intact installation.

SMB owns only the exact paths/hashes supplied by verified stable archives. Worlds/screenshots are never update targets. Mutable personal edits and deletions stay preserved, including defaults removed and reintroduced in later releases. Missing immutable files require explicit Repair pack; modified immutable files/unowned collisions need the player to move the conflicting file aside. Resource-pack synchronization must be disabled; native synced options/servers are excluded at adoption. Recovery journals block disabled helpers until an enabled check can recover safely.

## Local author export

The private packwiz authoring repository remains private. Export from explicit release metadata plus exact mrpacks, without copying adjacent server/source assets:

```powershell
python tools/export_minecraft_channel.py `
  --releases-json C:/author-input/releases.json `
  --input-dir C:/author-input `
  --current 0.13.0 --min-version 0.11.0 `
  --version 0.13.0 --version 0.11.0 --version 0.5.0 `
  --output C:/review/client/public --inventory C:/review/client/review.json
```

Input layout: `v0.13.0/smb-0.13.0.mrpack`, likewise each selected historical version. Metadata is an array of exact GitHub release objects with stable tag/draft/prerelease evidence and matching asset sizes/digests. Output must be a new directory; inventory must be outside it. Review inventory and exact embedded client bytes before any publication. Client jars retain ordinary mod contact/source URL metadata.

Proposed public feed: `https://raw.githubusercontent.com/jmpijll/smb-minecraft-client/main/stable.json`. Publish only the prepared public directory. Do not publish the private source, operational release files, review metadata or credentials. Future author releases must regenerate the channel with the separately supplied SMB_MODPACK minimum; never infer it from latest server release alone. Keep exact historical refs needed for existing installed versions.

For disposable testing only, `SMB_MINECRAFT_CHANNEL` accepts an explicit local stable.json. Production HTTP downloads require HTTPS and approved hosts; redirects are revalidated, reads are bounded, individual requests time out and staging has a ten-minute overall network deadline. Archive/download cache is outside profiles. SHA256 covers archive identity; SHA512 covers individual client files. No private token or LAN link API is used.

## Validation

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python -m unittest discover -s tools -p test_export_minecraft_channel.py
python -m unittest discover -s tools -p test_minecraft_cli.py
cargo build --locked --release
```

CLI tests use a disposable APPDATA/Modrinth database and real installed launcher version metadata. They enumerate helper windows and compare foreground handles; live Java activity correctly blocks later update tests, which are explicitly skipped rather than bypassed. Rust pipeline integration supplies a test-only busy predicate to execute adoption/archive/update twice against disposable files. Production process guards are never bypassed.

Opt-in tests (run individually): `minecraft::modrinth::tests::native_discovery_readonly`, `minecraft::service::integration_tests::actual_export_archives_match_channel`, `minecraft::service::integration_tests::actual_export_downloads_stage_only`. The last downloads real client mods into disposable staging only. Set an explicit local channel for the latter two. None launches Minecraft or mutates a real profile.

Actual egui captures are in this directory (compact layout matching the League page: one status card, one panel of switches, the rest under Instance & troubleshooting): [Minecraft ready](ready.png), [native setup](setup.png), [channel error](error.png), [League](league.png). All show synthetic states and are labeled Preview; no real settings/network/launcher actions run in screenshot mode. Default interactive window remains compact and scrolls.

## Boundaries to validate before broad member rollout

The native external launch URL and synchronous hook were checked against Modrinth 0.23.1 source, and the helper was exercised directly. An actual in-launcher hook/game-start session has not been performed while this machine's Java session is active. Later Modrinth versions intentionally fail until their schema/hook behavior is verified. Local filesystem checks reject existing reparse points, but are not a sandbox against a hostile process racing directory replacements. Busy checks do not claim to lock Modrinth's later Java startup.

## Decisions made during implementation

- Proceeded with reversible local work under the user's go-ahead, retaining an isolated branch. Cost if wrong: local changes can be revised without touching main.
- Chose a supported one-time native hook paste instead of private database edits. Cost: one setup step for direct launches.
- Limited initial automation to the same Minecraft/Fabric runtime. Cost: runtime migration remains manual in Modrinth.
- Retained shipped mutable-file tombstones across removed/reintroduced defaults. Cost: small retained ownership metadata, preserving personal edits/deletions.
- Kept exact stable client archive bytes, including ordinary private-repository contact URL metadata; source content/credentials are not published. Cost: that repository name is visible in client mod metadata if the client feed is approved.
- Prepared public client artifacts without publishing them. Cost: checked production launches report unavailable until the exact client feed is approved and published.
- Conservatively treated any Java process as busy. Cost: unrelated Java apps must close before pack updates, avoiding unsafe guesses about the game.
