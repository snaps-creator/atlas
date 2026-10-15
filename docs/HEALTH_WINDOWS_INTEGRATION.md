# Health and Windows lifecycle integration on 3.0.3

Date: 2026-10-15. Status: **DRAFT — Windows pin → upgrade acceptance is outstanding.**

## Base and scope

Target: `main`, fetched at `a243a40bdaeb276ef3718af60c1de60bb56db466`, identical to tag `v3.0.3` and the installed build. The source fixes came from the uncommitted `release/2.4.2` workspace at `eca2f9ed7ceeedf9e2ac70d6197340a20b819ec5`. Integration branch: `fix/health-windows-integration`, in a separate Git worktree. The original checkout and its unrelated changes were preserved. Package, Cargo and Tauri versions remain 3.0.3.

Recent merged PRs, remote branches and tags were inspected before integration. This change preserves the unified node repository, subscription refresh, startup coordinator, recovery state machine, patched tray implementation, application-owned latency scheduler and batched service IPC from 3.0.3. It does not restore the 2.4.2 React-owned probe timer. Routing/Gemini/TSPU experiments and unrelated `update_user.rs` changes are excluded.

The two earlier reports in this directory are historical evidence, explicitly marked as such. Their test counts are not the validation results for this branch. Local logs, machine snapshots, credentials, certificates, databases and generated executables are not committed.

## Resulting behavior

- A displayed success requires a real HTTPS control request, expected HTTP 204 and a verified concrete proxy outbound. Mihomo numeric history alone is insufficient. `unified-delay` is disabled so a successful warm request cannot conceal a failed measurement.
- Full Xray profiles retain their application routing and receive a separate authenticated loopback probe inbound pinned to a concrete outbound. DIRECT and selector-only evidence cannot certify a proxy.
- The shared probe store fences generations and operations, serializes the same node/URL, expires success after 10 seconds, retains failure during pending and separates HTTP 504 timeout from transport/controller failures. Completion time does not rejuvenate an older result. UDP/application connection errors remain separate evidence.
- The application scheduler retains bounded service batches, cancellation, background refresh and identity filtering. Connected reads use the core's latest certified evidence so a scheduler cache cannot resurrect success after recovery observes failure. Cards and ActiveServer consume the same UI snapshot. The active server no longer runs an independent history poll.
- Activation is forwarded to the pinned single-instance plugin before CEF startup. A startup barrier and pending activation cover first-launch races and pre-Ready callbacks. Tray and shortcut activation share show/unminimize/focus. Existing 3.0.3 manual-restart and visibility persistence remain intact; background/autostart relaunch does not become a manual activation.
- Process/window AppUserModelID, Shell relaunch properties and installer-created shortcuts use the existing product identity and stable root `Atlas.exe`. Shortcut stamping checks its target and is idempotent. Existing user taskbar pins are never rewritten.

## Validation

The integration was built and tested on Windows x64 with Rust 1.98.1, the repository's pinned Tauri/CEF dependencies and the current lockfiles.

| Check | Result |
| --- | --- |
| Frontend (Vitest) | PASS: 69 tests, 16 files; real React render includes timeout/pending, expiry, late results and recovery |
| TypeScript + Vite | PASS; existing large-chunk warning remains |
| Node contracts | PASS: 51 tests |
| Rust suite | PASS: 403 total (283 library, 50 Maintenance, 67 Updater, 3 desktop), 0 failed; 9 opt-in cases ignored in the default run |
| Background timer | PASS: one explicit 65-second loopback test; 7 automatic requests, no page-owned scheduling or user database |
| Windows native build | PASS: desktop, maintenance, updater; debug and embedded frontend (`tauri/custom-protocol`) |
| Real health integration | PASS: bundled Mihomo/Xray, local SOCKS and HTTPS origin; unauthenticated TLS rejected, trusted TLS HEAD/204 succeeds, stalled handshake/traffic time out, recovery succeeds; DIRECT false positive rejected |
| CEF lifecycle / shortcut / single instance | PASS: pre-Ready activation, visible/hidden/closed/minimized window, real temporary `.lnk` ShellExecute, shared tray handler, eight rapid launches, one focused window, no fixture processes left |
| Embedded CEF UI / IPC | PASS: four measured/timeout/stale/recovery phases, three page navigations, 25 buttons, normal CEF shutdown; timeout remains during pending |
| NSIS syntax | PASS with a dummy payload; generated test executable deleted without execution |
| Updater publication policy preflight | PASS as policy validation only; this does not establish release readiness |
| Diff/configuration/secret audit | PASS: no conflict markers or whitespace errors; no version changes, private keys, tokens, user configuration, databases or generated binaries included |

Commands used from the integration worktree:

```powershell
npm ci
npm test
npm run build
node --test scripts/*.check.cjs
. ./scripts/initialize-msvc.ps1
# CEF_PATH points to an existing local CEF cache; no installed Atlas files are used.
cargo test --locked --offline --manifest-path src-tauri/Cargo.toml -- --test-threads=1
cargo test --locked --offline --manifest-path src-tauri/Cargo.toml --lib -- --exact latency_refresh::tests::real_timer_cycles_without_a_page --ignored --nocapture
cargo build --locked --offline --manifest-path src-tauri/Cargo.toml --bins
cargo build --locked --offline --manifest-path src-tauri/Cargo.toml --bins --features tauri/custom-protocol
./scripts/test-windows-lifecycle.ps1
$env:ATLAS_LATENCY_UI_SMOKE='1'
./scripts/test-ui-startup.ps1 -Executable ./src-tauri/target/debug/atlas-vpn.exe
git diff --check
```

Initial failures were investigated rather than counted as passes: Windows sandbox blocked Vite cache rename and MSVC PDB operations; commands succeeded outside that sandbox. Earlier recovery/controller fixtures lacked certified health records and expected the old timeout label. Their protocol and assertions were updated. Recovery backoff fixture now establishes a previously observed failure before asserting that repeated failures preserve backoff; a genuinely new outage still preempts it. The first UI smoke used a dev-URL binary and timed out; the embedded frontend build passed. NSIS required the same UTF-8 BOM used by the production build script.

PR #65's first CI run failed in the HTTPS fixture before any probe: a clean checkout had no ignored `temp` parent, and `create_dir` returned Windows error 3. The fixture now uses `create_dir_all`. The exact failing test passed locally with the worktree's `temp` directory explicitly absent at test start; existing evidence was preserved and restored. The GitHub rerun must still be assessed independently.

## Windows update acceptance and limitations

No accessible local disposable VM was found: the host is a physical MSI machine; Hyper-V management/Get-VM, VMMS, VMware and VirtualBox tools were unavailable. HypervisorPresent alone does not provide a test VM. No test installation, production updater, service restart, TUN/DNS/proxy modification or release publication was performed.

Required before merge/release in an isolated interactive Windows VM:

1. Install a previous supported release (include the user's known-good 3.0.1/3.0.2 and current 3.0.3), pin it, and record the pin's target and AppUserModelID.
2. Upgrade through the normal signed updater/installer, not by replacing executables.
3. Verify the existing pin after old version cleanup, Desktop and Start Menu links, hide/minimize → restore, and repeated launch without a second application/VPN instance.
4. Reinstall the same version and repeat the checks.

COM/unit/NSIS tests do **not** establish this acceptance. Legacy pins pointing at a versioned executable are not automatically migrated by this change; their survival must be tested. Existing GitHub connected-upgrade automation uses legacy/2.4.2 baselines and does not perform this taskbar-pin sequence. CI results must be assessed separately for this PR's actual head.

Other limits: live public subscriptions and Gemini access were not exercised by this integration; loopback fixtures certify the stated control path, not UDP/STUN or every application destination. Elevated/unelevated activation mismatch, other Windows sessions/desktops and physical reboot/power-loss upgrade recovery remain untested. A mismatched older service that cannot return certified probe records fails closed rather than displaying unverified latency; the installed pair must be upgraded together through the normal updater.

## Host preservation

Read-only before/after comparison passed: all 7 installed process IDs and creation times unchanged; AtlasNetworkService remained running; Atlas-TUN interface 2 remained Up; route, DNS and system-proxy fingerprints unchanged. The source checkout remains `release/2.4.2` at its original HEAD. This work never executed its production setup.
