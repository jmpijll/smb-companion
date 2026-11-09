# SMB Minecraft automatic pack updates

## Agreed intent

Jamie asked to start implementation after choosing Minecraft as the next companion capability. League preparation stays intact. Minecraft should keep one SMB Modrinth instance current, preserve personal settings/saves, and support Play SMB plus direct Modrinth launches, with toggles. iRacing spotter setup, overlays and paints are later work.

Execution go-ahead is the user's “Ok lets get it going”; routine reversible implementation decisions are delegated to us. Publication of a previously private client pack needs a concrete final visibility decision; do not publish private source or server/ops artifacts as a shortcut.

## Approaches and decision

1. Publish a managed Modrinth project and reuse native updates. Native updating exists, but its apply-to-existing-instance call is internal Tauri IPC; the external protocol cannot synchronously update the selected instance. It would also require project moderation and publishing credentials.
2. Consume a verified client-only release channel exported from the existing GitHub mrpacks, synchronize pack-owned files in one selected native Modrinth instance, then use Modrinth's own launch protocol and pre-launch hook. **Chosen implementation approach:** this preserves existing authoring/release flow and both entry points without injecting into a WebView or editing launcher databases.
3. Open each new mrpack in Modrinth. Rejected: it creates another instance rather than updating the selected one.

## Proven launcher boundary

Installed Modrinth 0.23.1 source commit e949f1b2e73853e83ffdc9541bdb5aa5d5a660f1 supports `modrinth://launch/instance/{percent-encoded canonical ID}`. Pass it as one argument to the executable. A per-instance pre-launch command is parsed by shlex, receives INST_DIR, waits, and blocks launch on nonzero exit. Files are rescanned after this hook. The Minecraft/Fabric/Java context is loaded before the hook, so this feature **must fail before changing files if the target Minecraft/loader differs**. Runtime migrations are handed back to native Modrinth; no invented external update API.

Use the launcher database read-only for instance identity, directory, applied runtime, imported pack version and hook verification. Never query account/token/cookie data or write private database internals. Recognize the 0.23 schema; unsupported schemas fail helpfully. Respect a custom configured profiles directory only if discovered from the supported profile query; never guess by duplicate display names. A one-time native Hooks setting is needed for direct launch: companion offers Copy launch hook, exact short instructions and readback verification. It must not label the integration active before that hook is actually saved. Existing custom hooks are left unchanged.

## Release authority and delivery

The private `jmpijll/smb-minecraft` packwiz tree is authoritative. Only stable tagged releases are candidates, never main/edge/dev. Its SMB_MODPACK is the oldest compatible client pack, separately from server release. The publisher prepares a client-only channel JSON and verified mrpacks; no compose/env/jar server assets or repository source are included. A server-only release retaining the same required pack does not force replacement of a compatible installed pack.

Public delivery location proposed for final approval: `jmpijll/smb-minecraft-client`, containing only stable channel metadata and client mrpack releases. Source repository remains private. Until publication is approved, prepare exact export files locally and support a local channel path for testing; production UI honestly reports channel unavailable. Never embed a private GitHub token in the companion. Do not expose the server's LAN link API.

Channel schema 1: `pack` (version, min_version, url, sha256, size), `history` (same refs for adoption), `minecraft`, `loader`=`fabric`, `loader_version`. Stable three-component numeric versions only. Production network sources are HTTPS, bounded and timed out. Old profile adoption requires the exact prior mrpack from trusted history to establish ownership. Unknown/dev versions are not silently adopted; the player can import the approved pack once in native Modrinth. Archive SHA256 and every downloaded file's mrpack SHA512 must match before commit.

## Profile update contract

- Target one explicitly selected canonical Modrinth instance. Save selection in a separate Minecraft settings file under existing companion data directory; do not reset League preferences, install identity or portable behavior.
- Automatic updates default on. Play SMB waits for successful check/update then hands off to Modrinth; “handed to Modrinth” is not proof Minecraft launched. A launch-after-update toggle changes the primary action to update only.
- Direct-launch-check toggle controls a headless `--minecraft-prelaunch` mode, handled before single-instance GUI logic. It reads INST_DIR, verifies it equals the configured selected instance and uses the same updater. Disabled mode returns zero without work. Failures return nonzero, save a plain-language status/log, and never open a window.
- One per-profile cross-process lock serializes both entry points; no detached updater returning zero early. Bounded network operations and a global operation deadline prevent indefinite hooks.
- Check for Java/Minecraft processes before modifying any instance files. Defer if present, with an honest “Minecraft or another Java app is running” explanation. No app self-update installation during Minecraft/Java activity or pack work.
- Stage all files first. Apply replacements by rename, never by in-place modification of possibly hardlinked Modrinth content. Keep an interruption journal and backups; recover to a complete prior installation before another attempt. A failure never marks the new version installed.
- Track exact pack-owned paths/hashes. Remove only obsolete owned immutable files whose contents match the baseline. Preserve user-added files. Modified immutable pack files cause an actionable conflict instead of being overwritten. Mutable config/options are updated only when unchanged from the old pack; player-modified settings remain intact. Saves, screenshots, accounts, launcher metadata and unrelated profiles are never targets.
- Allowed targets: mods/, config/, resourcepacks/, shaderpacks/, and root defaults options.txt/servers.dat. Validate every path against Windows traversal, absolute/drive/UNC/ADS/device names, case collisions, symlinks and junctions. Reject archive escapes, server-only downloads and oversized content.
- Durable state lives in the selected profile's `.smb-companion` directory; archive/download cache belongs in companion data. No cache or state writes in demo/screenshot mode.
- Keep a compatible pack through a server-only bump; permit an explicit repair/update action for the latest pack. Never downgrade a newer local version. Runtime mismatch is an explicit native Modrinth action, not an attempted launch of incompatible mods.

## UI and focus

Use existing Rust/egui SMB tokens/fonts/controls, with a compact League/Minecraft selector and SMB // COMPANION heading. League remains the default existing context. Minecraft has one hero status, selected profile, pack state, primary Play SMB/Update pack action, the two update/launch preferences and direct-launch setup status. Show progress in-window and passive status only. Advanced diagnostic/source details are collapsed. No fabricated connected/ready state.

Do not auto-launch either game on application startup, detection or update completion. Opening Modrinth is allowed only for a player's Play/import/setup action. No notifications, overlays, sounds, taskbar flash, always-on-top or automatic companion focus. Minecraft headless hook never creates the GUI or grabs the existing instance window. Existing League worker/tray behavior continues independently. Autostart/executable identity stays compatible with existing installed users; don't prematurely rename the package or updater repository.

## Validation and handoff

Baseline: 38 current tests pass, one live release test ignored. Add meaningful real filesystem/archive/local-HTTP tests for ownership, preserved personal files, removed mods, corrupt hashes, traversal, concurrent update locks, interruption recovery, busy process deferral and runtime mismatches. Read-only profile discovery and exact encoded launch/hook commands are tested with fixture databases. Test headless failure produces nonzero without GUI; current user profiles are never test fixtures. Capture actual rendered League and Minecraft windows in demo mode, including failed/setup-needed states. Build/review locally before any publication or release.

Required final handoff: reviewable feature branch/build, test evidence, screenshots and exact client-only delivery bundle. Any public feed creation/publication is the final explicit visibility step, with only the prepared files shown for approval. No server deployment, survival-world change or source-repository visibility change.

## Builder review rulings

- Mutable ownership is a three-way comparison: persist NEW SHIPPED hashes after preservation, never the player's changed bytes. Previously deleted mutable defaults stay deleted; missing immutable owned files block automatic launch until explicit repair. Existing unowned collisions are never overwritten.
- Runtime belongs to each version reference and is cross-checked against the archive. Resolve compatible installed pack before choosing latest target; a newer optional runtime cannot invalidate a verified compatible older pack. Refs carry Minecraft, loader and loader_version in addition to archive identity.
- Read-only adapter requires tested Modrinth 0.23.1 executable version plus recognized schema. Native synced game options/resources are excluded from updater ownership; only mods and explicitly unmanaged defaults can be updated. Existing effective global/per-instance hook customization is detected and not overwritten or implicitly dropped.
- One OS-backed operation lock is acquired BEFORE authoritative state read, recovery, adoption and staging, and held through commit. `ProfileLock` is passed to the transaction module; no nested locks. Disabled helper checks for pending journal and returns nonzero if inconsistent, rather than authorizing a broken launch. Release lock before Modrinth handoff; hook checks freshly. Busy recheck does not claim a lock spanning native Java spawn.
- Public export is an exact reviewed stable archive allowlist with filename/size/SHA256 inventory, version/runtime checks and no adjacent source/admin attachments. Approval binds to those bytes. No public visibility action is performed during implementation.
