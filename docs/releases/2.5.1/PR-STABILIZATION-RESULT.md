# PR stabilization result

Existing PR: [#60](https://github.com/snaps-creator/atlas/pull/60), release/atlas-2.5.1 -> main.
Initial SHA: d0cf52a9404229b7360115f1a495c7369c50a0e8.
First correction SHA:8c32a80d5a953fffa55593d6b65fbd1804c239f7. Second correction SHA: pending batch commit. The exact final commit must match PR head, CI head_sha and signed artifact build/manifest. A file cannot contain its own commit hash; the explicit verified SHA is recorded in the PR description and final delivery.

Current status: **BLOCKED BY CI**. First correction's main CI passed, but three installed jobs failed the tray interaction. The complete matrix and all failed logs were collected before starting the second coherent correction batch. CI outcomes belong only to the exact head that produced them.

## Findings and correction batch

The [issue register](PR-STABILIZATION-ISSUES.md) groups 14 completed failed CI jobs across seven runs into six confirmed primary causes (S01/S02/S03/S04/S05/S08), independent warning/data coverage defects S07/S11, uncertain CEF frame failure S06, style/residual limitations S09/S10/S13 and unavailable local gates S12. C01/C02/C03 are downstream failures, not extra root causes. S14 records three failures from the first local batch native run and is not reproduced in the complete serial run; all original failures remain recorded.

Prior commits already fixed baseline-version/schema assumptions and post-uninstall data verification. The local correction prepares a medium-integrity shell before tested tray creation, checks native icon/cursor prerequisites, propagates smoke acknowledgement/report failures to process exit, clears only the monitor's own stale warning, removes unused frontend declarations and enables strict unused checks. Native main-bin smoke regressions are now part of both mandatory workflows. New files/modules receive scoped formatting; inherited global formatting debt is disclosed.

## Validation evidence

- Frontend: 51 tests PASS; production build and strict unused TypeScript PASS. Vite retains its existing chunk-size advisory.
- Contracts: 41 PASS, including runner guard and extracted native tray declaration compilation.
- Migration fixtures: both actual baseline schema branches PASS, preserved names/references/backups checked and tampering rejected.
- Syntax: 37 PowerShell scripts, 10 JSON files, 2 TOML files, Cargo.lock, Python AST and all 6 workflow graphs PASS.
- Native: cargo check all targets PASS; Clippy exit0 with assessed inherited style warnings. First batch full suite:244 library PASS/3 FAIL/1 ignored;50 maintenance PASS;65 updater PASS/3 ignored;3 main-bin smoke PASS. The original log is retained; unchanged full serial confirmation after all competing builds finished:247 library +50 maintenance +65 updater +3 main=365 PASS,0 FAIL; intentional ignores remain1+3. Contention is a hypothesis; the original3 failures are not erased.
- Scoped rustfmt/Prettier PASS. Global inherited format warnings are not called PASS. npm dependency graph PASS and audit0 vulnerabilities in the diagnostic phase; dependencies unchanged.
- Full installer build/extraction/hash/native receipt/corruption validation: PASS:32files, actual PE/version metadata, complete offline composition and corrupted payload rejection. Local EXE190463957bytes/SHA256cc0871db33c1818ec20ca6d08f3dec395c4b97d9f1b9a775da311720fa10c21d. Actual packaged UI positive and unwritable-report negative PASS:render/IPC, normal lifecycle, exit2 on failed durable report and no remaining helpers. Local unsigned warm-target output cannot establish clean signed final-head evidence.

## CI, review and environment

Initial-head main checks PASS at [run38078011573](https://github.com/snaps-creator/atlas/actions/runs/38078011573). Signed build and actual2.4.3 baseline build PASS; all installed matrix outcomes at [run38078011569](https://github.com/snaps-creator/atlas/actions/runs/38078011569) completed before correction push:legacy PASS;2.4.2 and2.4.3 FAIL solely S04 trayRestart. All A–H startup and real UI disconnect/reconnect PASS. Candidate remediation checks PASS; installed remediation FAIL solely sameS04. All three full failed logs were reviewed; no new root cause. These checks do not validate uncommitted corrections.

At diagnosis: mergeable=true; refreshed main365b91279f5b7c1a9fd49fd49ab42f1b5f986c69 unchanged; review/review comments/unresolved threads0 with complete pagination. Main ruleset23691068 requires PR/resolved threads and forbids non-fast-forward/deletion; named required-check array is empty, but task checks remain mandatory. Recheck all on final SHA.

Workstation installation, live VPN/DNS/routes/proxy and existing user configuration were not modified. Installed TUN/SCM/startup/tray/rollback/uninstall gates remain confined to disposable Windows CI. Local admin restricted-child invocation, physical reboot/power loss and physical Wi-Fi switch are NOT RUN. actionlint/ESLint are unavailable/not configured; available syntax/type/compiler checks are listed separately. Audit006 IPC p95,015 hidden CEF benchmark and016 public privacy export remain explicitly open.

No merge, release tag or publication is performed. Public distribution remains GitHub Releases; official latest is2.4.2 until an independently approved publication. Installation EXEs/parts are excluded from Git. User changes in the original checkout are preserved.

Corrected problems: priorS01/S03/S11, currentS02/S04/S05/S07/S08 plus formatting/spacingS09/S10 (10 rows addressed; some require final installed CI to establish closure). S06 remains conditional final smoke gate; S12/S13 are explicit availability/residual limits. Final CI/readiness evidence is recorded for the exact correction SHA in the linked PR description, current-head GitHub Actions and signed build/manifest; this committed document is the completed local-validation snapshot, not a claim of future CI success.

## First correction outcomes and second correction

At8c32a80: [main38081178645](https://github.com/snaps-creator/atlas/actions/runs/38081178645) SUCCESS, including366 native tests with restricted-process check. [Connected38081178662](https://github.com/snaps-creator/atlas/actions/runs/38081178662): signed full installer and actual2.4.3 baseline builds PASS; legacy full acceptance PASS; published2.4.2 and actual2.4.3 installed jobs FAIL only tray GetRect interactionS04. [Remediation38081175340](https://github.com/snaps-creator/atlas/actions/runs/38081175340): clean signed build/backend/package/VerifyOnly PASS; installed job FAIL sameS04. All A–H and real UI disconnect/reconnect checks preceding tray interaction PASS. No other completed failure found.

The initial shell replacement hypothesis did not establish closure. S04's previous FIXED claim is withdrawn. The current helper ran elevated while actual app and Explorer ran medium. Second batch launches a verified same-user/session medium non-admin helper and asserts matching integrity. GetRect records exact HRESULT for each bounded identifier; a failed query is no longer misreported as proof of missing registration. Exact causality of the prior GetRect failure remains subject to installed CI.

An additional real defectS15 is independently reproduced twice: UIAutomation Invoke returns successfully on the pinned native tray menu without invoking its handler. The corrected helper uses actual native input on the verified owner's enabled MenuItem, with cursor restoration. A standalone pinned tray fixture exercises the same implementation and requires a callback acknowledgement plus normal process exit; this test is mandatory in both workflows. Installed acceptance additionally requires a real new Atlas PID and unchanged durable state. No checks, deadlines or retries were weakened; no production application code changed in this second batch.

The8c32a80 signed installer is independently verified but is not a final release candidate:190500431bytes, SHA2568284770b35f6a2244f1d53120aa3c1441bc2e15237c2c3c61f1679ecf980954f. Exact build SHA, archive digest, minisign signatures of installer and manifest, all33 payload hashes/sizes and native updater verification PASS. Installed acceptance FAIL means overall readiness remains blocked.

Second complete local gate:51 frontend tests,41 contracts, strict production build, cargo check all targets,38 PowerShell syntax checks, JSON/TOML/locks/Python parsing, six workflow graphs, both migration/tamper fixtures and actual standalone tray callback/normal-exit test PASS. Fixture locked offline build uses application lockfile versions/checksums; helper native declarations compile. Clippy exits0 with previously assessed inherited warnings. Full serial backend:247 library +50 maintenance +65 updater +3 main=365 PASS,0 FAIL,1+3 intentional ignores unchanged. Scoped fixture rustfmt/workflow Prettier and diff whitespace checks PASS. Production dependencies and binary sources are unchanged. First-correction signed package verification remains valid only for8c32a80; next-head clean signed build and all installed CI are required.
