# Minecraft Companion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement task-by-task.

**Goal:** update one SMB Minecraft Modrinth profile safely before either supported launch path, retaining the existing League companion.

**Architecture:** standalone testable pack transaction module, bounded channel/archive/download service, read-only Modrinth adapter, headless prelaunch CLI and existing egui window. Prepare a client-only stable release feed from the private authoring repo; publish only after visibility approval.

**Tech Stack:** Rust/egui, existing ureq/ring/serde_json, zip archive parser, read-only rusqlite, filesystem locking. Python publication export tool.

**Spec:** ../specs/2026-11-09-minecraft-companion-design.md

## Global Constraints

- Windows; existing League settings, installed identity and portable mode preserved.
- Automatic updates default on; launch-after-update and direct-hook preferences explicit.
- Modrinth 0.23.1 external launch/hook contract; no private IPC injection or database writes.
- Match Minecraft/Fabric runtime before any changes; no token shipped and no public source/ops artifacts.
- Never change a running instance; headless helper never opens/focuses UI; no unprompted launch.
- Stage/verify before commit; preserve personal data; interruption recovery and per-profile lock.

## Review Focus

1. Old pack ownership and mutable personal config must survive adoption/update.
2. Archive paths/hash failures and NTFS links must never escape the selected profile.
3. Two entry points and interrupted commits must not produce mixed pack state or false success.
4. Runtime mismatch, offline source and missing native hook must be honest and launch-blocking where required.
5. Existing League worker/preferences and no-focus behavior must remain intact.

### Task 1: Transactional profile synchronization

Files: src/minecraft/pack.rs; pure tests in that module.

Interfaces: `PackFile {path: String, bytes: Vec<u8>, mutable: bool}`; `InstallReport {written: usize, removed: usize, preserved: usize}`; `ProfileLock::acquire(profile: &Path) -> Result<Self,String>` with bounded wait; `ProfileLock::profile(&self)->&Path`; `apply(lock: &ProfileLock, baseline: &BTreeMap<String,String>, files: &[PackFile], version: &str, repair: bool, busy: impl Fn()->bool) -> Result<InstallReport,String>`; hashes are SHA512 lowercase hex. State/journal/lock under .smb-companion. `recover(lock, busy)`, `load_version(lock)->Result<Option<String>,String>`, `owned_files(lock)->Result<BTreeMap<String,String>,String>` and `has_pending(profile)->Result<bool,String>` support orchestration. Lock acquired before authoritative reads; apply never nests it. Pending journal blocks disabled helper. Mutable baseline is new SHIPPED hash, preserving local edits across successive updates; missing owned immutable files require explicit repair, missing mutable defaults stay absent.

- [ ] Write tests first proving preservation, exact removal, unchanged defaults updated, modified defaults preserved, conflicts fail before commit, blocked processes do not mutate, unsafe paths and symlinks rejected, lock serialization and crash recovery.
- [ ] Observe failures, implement the bounded transaction, and run all tests.
- [ ] Review module against spec, record evidence and commit only its files.

### Task 2: Channel, mrpack and launcher adapter

Files: src/minecraft/mod.rs, channel.rs, modrinth.rs; Cargo.toml/lock; tools/export_minecraft_channel.py.

Interfaces: worker consumes immutable Settings/Instance snapshots; core apply interface above. Channel validates stable semver, current/minimum relationship, exact archive and file hashes, limits and runtime. Adapter returns canonical instance identity/path/runtime/imported version/hook match, using readonly scoped SQL. Launch URL is native encoded canonical ID, never a shell command. Hook command uses quoted forward-slash executable and --minecraft-prelaunch; instance directory comes from INST_DIR.

- [ ] Add failing fixture tests for malformed channels, compatible server-only release, version downgrade prevention, runtime mismatch, exact mrpack client layering, ignored server-only files, URL/path/hash failures and launcher schema/selection/quoting.
- [ ] Implement bounded fetch/cache, verified prior-pack ownership and current file staging; expose operation result through shared status and commands.
- [ ] Add author-side export tool taking stable releases and emitting only channel JSON/client archives; test it with fixture release metadata and archives, then prepare real private-source export locally without publishing.
- [ ] Run the focused and full suite; record source/version boundary and network limits.

### Task 3: Headless CLI and compact companion UI

Files: src/main.rs, src/app.rs, src/system.rs, src/update.rs, README.md; screenshot mode and integration tests.

- [ ] Add failing command/guard tests: helper path precedes GUI single-instance logic; wrong/absent selected directory fails, disabled helper succeeds without writes; encoded handoff; Minecraft/Java blocks self-update.
- [ ] Wire worker channels without UI-thread network IO, compact game selector, setup/ready/progress/error states, persistent independent settings and native-hook copy/readback instructions. Keep League context and worker unchanged.
- [ ] Add Minecraft demo/capture mode; capture real egui screens and compare brand/spacing against current app. Demo mode cannot save real settings or run network/launcher actions.
- [ ] Validate headless exit/no-GUI with disposable instances, launcher read-only discovery on current machine, all tests, format/clippy/release build.

### Task 4: Review and concrete distribution handoff

- [ ] Fresh whole-branch review of safety, user flows and no-focus regression; fix findings and rerun affected checks.
- [ ] Produce local executable/portable review build plus labeled actual screenshots and client-only export file inventory.
- [ ] Report exactly what is tested and the native runtime/hook-setup limitation. Ask only for the final public artifact visibility/publishing action if it is still not authorized; keep private source and server untouched.

## Implementation ledger

- Baseline tested in isolated root worktree feat/minecraft-companion:38 passed/1 ignored.
- Ruling: user go-ahead authorizes reversible implementation of the discussed behaviors; no repeated permission gate for local code/design steps.
- Ruling: initially use supported native hook setup with readback, not automatic private launcher DB edits. Cost: a one-time paste in native Modrinth settings.
- Ruling: Minecraft/Fabric migration blocks explicitly; same-runtime updates are the first native-supported automation boundary.
- Public client delivery remains a concrete final visibility step; no publication yet.
- Builder review completed with no remaining product question. Incorporated three-way baseline, per-ref runtime, exact launcher boundary, native-sync exclusion, operation-wide lock/pending gate and reviewed public artifact inventory.
