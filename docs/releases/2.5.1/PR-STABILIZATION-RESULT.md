# PR stabilization result

Existing PR: [#60](https://github.com/snaps-creator/atlas/pull/60), release/atlas-2.5.1 -> main.
Initial stabilization SHA: d0cf52a9404229b7360115f1a495c7369c50a0e8.
Last fully diagnosed CI SHA: dad195f14b2c91c740ab1acf62e7637cf3389b1a.
Current status: **BLOCKED BY CI**. This is the third correction's local-validation snapshot, not a claim that its future CI has passed. The final commit is bound to PR head, CI head_sha and signed artifact build/manifest; its explicit SHA and final outcomes are recorded in the PR description and delivery evidence.

## Diagnosis and corrections

The [issue register](PR-STABILIZATION-ISSUES.md) covers 21 completed failed jobs across 12 failed runs. It tracks 17 issue rows and 4 downstream outcomes C01–C04. Seven confirmed primary causes are S01/S02/S03/S05/S08/S15/S17. S07/S11 are independent correctness/coverage issues. S04/S06/S14/S16 retain the uncertainty described in the register. S09/S10 are assessed style warnings; S12 documents unavailable environments; S13 records open audit improvements.

Nine correctness/coverage rows have implementation changes retained or completed: baseline version S01, shell bootstrap S02, schema assumptions S03, smoke exit outcome S05, stale warning ownership S07, unused frontend declarations S08, post-uninstall data assertions S11, actual native menu input S15 and runner environment propagation S17. Scoped formatting/spacing S09/S10 is also addressed. This count does not claim final installed CI closure.

The first correction 8c32a80 passed main CI but still failed three installed tray interactions. The initial shell hypothesis was insufficient; S04's previous FIXED claim was withdrawn. The second correction dad195f exposed two separate problems: its minimal native fixture fails a combined UIAutomation lookup/type/enabled condition on Server 2025; the new limited helper loses runner variables because CreateProcessWithTokenW receives a NULL environment and creates a profile environment. Both installed failure artifacts contain no transcript because the helper guard runs first.

The third batch passes the caller's native Unicode environment block with CREATE_UNICODE_ENVIRONMENT and releases the block after process creation. Medium non-admin user/session checks remain mandatory. The native popup driver uses documented HWND/HMENU APIs, exact owner and label, enabled state, current bounds and a hit-test before actual input. Both fixture and installed helper use the same implementation. The fixture requires separate tray callback evidence, native command acknowledgement and normal exit, and rejects a wrong PID, wrong label and real disabled command. Installed acceptance still requires a new Atlas PID and preserved durable settings. No application code, dependency version, test deadline, retry or security gate is relaxed.

## Local validation

- 41 Node contracts PASS, including native declarations compiled using Windows PowerShell 5.1, the actual limited-helper runtime.
- 51 frontend tests PASS; strict unused TypeScript and production build PASS. Existing Vite chunk-size advisory remains.
- Cargo check all targets PASS; Clippy exit 0 with previously assessed inherited warnings.
- Full serial native suite: 247 library + 50 maintenance + 65 updater + 3 main = 365 PASS, 0 FAIL. 1+3 intentional ignores remain unclaimed; real restricted-process acceptance belongs to CI.
- Native tray fixture PASS: registered rectangle, tray callback, real native popup, owner/label/enabled/hit-test checks, negative cases, actual input and command callback/normal exit.
- Both baseline schema migration/preservation/tamper fixtures PASS. 38 PowerShell parsers PASS. All 10 tracked JSON files, 5 TOML/lock files, one Python source, 6 workflow dependency graphs and scoped formatting checks PASS.
- Full local installer build and verification PASS: Atlas-Setup-2.5.1.exe, 190454302 bytes, SHA256 `97eb39a123194178e082e9f005747c6f0ca9a918092cc7a15810ffe90f7ba04b`; all 32 files, native x64/version metadata, runtime/core composition, embedded receipt and corrupted/restored payload checks PASS. This is an unsigned warm-target local build (`local-stabilization-dad195f`), not final-head clean signed CI evidence. Installer was not executed.
- Actual UI from the extracted package PASS: React rendered 12 buttons, IPC acknowledged, normal completion and no remaining owned helpers. Deliberately unwritable report PASS: exit 2, exactly one failure/completion flow, no remaining helpers.

The first 8c32a80 local native run had 3 process/network fixture failures while release compilation was running. The unchanged complete serial run and the next two complete serial batches pass; original failed logs remain retained and contention remains a hypothesis. No deadline was increased.

Local AppContainer initially ran Windows PowerShell 5.1 at low integrity 4096 and denied the schema fixture's temporary-file write. Those same tests pass in the approved ordinary medium context, with assertions unchanged. Actual token-based launcher execution is NOT RUN locally: CreateProcessWithTokenW returns Win32 1314 under the non-admin user. No UAC elevation was requested; installed CI must prove that path.

## Completed CI evidence

| Head | Main | Connected upgrade | Independent remediation |
|---|---|---|---|
|8c32a80|[PASS](https://github.com/snaps-creator/atlas/actions/runs/38081178645), including 366 native tests with restricted child|[FAIL](https://github.com/snaps-creator/atlas/actions/runs/38081178662): clean signed build and actual 2.4.3 baseline PASS; legacy PASS; 2.4.2/2.4.3 fail tray GetRect S04|[FAIL](https://github.com/snaps-creator/atlas/actions/runs/38081175340): candidate/package/backend PASS; installed fails same S04|
|dad195f|[FAIL](https://github.com/snaps-creator/atlas/actions/runs/38085305950): frontend/contracts/backend/restricted child PASS; native fixture fails S16|[FAIL](https://github.com/snaps-creator/atlas/actions/runs/38085305902): clean signed build and actual 2.4.3 baseline PASS; legacy PASS; 2.4.2/2.4.3 fail limited helper before transcript S17; A–H otherwise PASS|[FAIL](https://github.com/snaps-creator/atlas/actions/runs/38085304037): clean build/backend/package/VerifyOnly PASS; native fixture fails S16; installed skipped C04|

All four dad195f failed job logs, the native fixture artifact and both installed failure artifacts were collected before the third implementation correction. Archive digests match GitHub metadata. This complete collection is the basis for the current batch; no intermediate fixes were pushed during diagnosis.

The 8c32a80 signed installer was independently verified: 190500431 bytes, SHA256 8284770b35f6a2244f1d53120aa3c1441bc2e15237c2c3c61f1679ecf980954f; exact SHA, archive digest, installer/manifest minisign signatures, 33 payload hashes/sizes and native updater package/payload verification PASS. This is historical integrity evidence, not a final candidate for a different SHA. Every next-head signed build and installed matrix remains mandatory.

## Review, protection and remaining limits

Refreshed main is 365b91279f5b7c1a9fd49fd49ab42f1b5f986c69, unchanged. GitHub reports mergeable=true. Reviews, review comments and issue comments are 0 with complete pagination; previous unresolved thread inventory is 0 and must be refreshed before final readiness. Main ruleset 23691068 requires a PR/resolved threads and forbids deletion/non-fast-forward; named required-check list is empty, but every task gate remains required.

Physical reboot/power loss, physical Wi-Fi switching and offline-hive logon races are NOT RUN. Audit 006 IPC p95, 015 hidden CEF benchmark and 016 public privacy export remain open improvements. actionlint/ESLint are unavailable/not configured. Inherited Clippy/global formatting debt is disclosed. Authenticode signing is not claimed; updater/minisign signatures and authenticated manifests are checked.

No merge, release tag or publication is performed. Public distribution remains GitHub Releases and the existing latest 2.4.2 until separate publication approval. Installers/parts are excluded from Git. The original checkout's uncommitted changes, working VPN, DNS/routes/proxy and user configuration are preserved.
