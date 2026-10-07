> Historical 2.4.1 checkpoint. Current implementation and observed validation: [RELEASE-2.4.2.md](RELEASE-2.4.2.md).

# Atlas updater implementation — 2026-10-07

Статус: **работа не завершена; releaseReady=false**. Этот документ заменяет устаревший отчёт «фазы 1–10 не начаты». Предыдущая сборка 2.3.2-alpha.1 не является подтверждением текущих исходников 2.4.1. Установка на рабочую систему, публикация, push, merge и PR не выполнялись.

## 1. Final architecture

Реализованы native AtlasUpdater без CEF, потоковая загрузка, проверка подписанного manifest/package, неизменяемые каталоги версий, атомарный current.json, machine-wide mutex, durable admission fence, state machine, orchestration rollback, native health contract и SQLite snapshot/restore. Кандидат до commit не обновляет подписки и не принимает изменения настроек/подключения. Startup и broker admission удерживают update lock; после аварии процессная блокировка дополнена проверкой журнала.

**Архитектура ещё не замкнута в production pipeline.** AtlasUpdater CLI предоставляет protocol и verify-package. Windows SCM adapter реализован, но не связан с concrete Platform; stable launcher/bootstrap и автоматический запуск recovery после reboot не реализованы. UI и NSIS намеренно отклоняют небезопасное обновление. Это временная защита от повреждения, а не законченная функция обновления.

## 2. Changed files

- `src-tauri/src/update_transaction.rs`, `update_lock.rs`, `update_supervisor.rs`: durable authority, immutable staging, последующие транзакции, история, rollback и запрет новых сессий.
- `update_integrity.rs`, `update_download.rs`: public-key trust, подписи, pin verified file, ограничения metadata, streaming/resume/cancel.
- `update_health.rs`, `update_candidate.rs`, `update_process.rs`: bind transaction/nonce/PID/version/build, ограниченный IPC, реальный дочерний процесс и stability window.
- `update_data.rs`, `storage.rs`: SQLite WAL-consistent backup, schema guard и идемпотентное восстановление.
- `update_service.rs`: SCM ImagePath-only switch с сохранением остальных параметров.
- `update_log.rs`: typed structured events и исключение секретов.
- `lib.rs`, `broker.rs`, `subscription_refresh.rs`: admission и health-candidate mutation guards.
- `maintenance.rs`, `installer-hooks.nsh`, capabilities, `src/main.tsx`: final network-baseline check и fail-closed legacy install.
- build/verification scripts, release workflow, pinned toolchain, public crypto fixtures и `updater-readiness.json`.

Существующие изменения подписок, VLESS, source switch и восстановления сети сохранены.

## 3. Closed risks

Статус PASS ниже относится только к указанному проверенному механизму. Он не означает готовность updater целиком.

| ID | Problem | Root cause | Fix | Test | Result | Evidence | Status |
|---|---|---|---|---|---|---|---|
| U01 | In-place replacement | Legacy plugin/NSIS path | Install capability удалена; UI blocked; GUIINIT и PREINSTALL отклоняют upgrade, включая /D | Permission и NSIS regression | PASS static checks; GUI execution pending | `temp/updater-js-tests.log` | MITIGATED, transactional replacement INCOMPLETE |
| U02 | Postinstall error unnoticed | Ignored sc.exe results | Ненулевые exit codes, abort | Script checks; package compile pending | Код изменён; fresh-install rollback отсутствует | `installer-hooks.nsh` | OPEN |
| U03 | Unhealthy candidate accepted | No identity-bound health | Native child IPC/nonce/PID/build, real UI callback, stability, generic rollback | Wrong nonce, crash, health failure | PASS isolated; SCM/CEF combined activation absent | `temp/updater-final-regression.log` | PARTIAL |
| U10 | Crash leaves mixed state | No durable authority | Atomic journal, immutable versions, conservative recovery, history | Kill process on 8 stages; rollback restart | PASS process/filesystem; Windows reboot not tested | `temp/updater-process-crash-tests.log`, final regression | PARTIAL |
| LOCK | Reconnect after updater crash | Abandoned mutex accepted as admission | Persistent stage/active-version fence | Critical stages, wrong desktop after commit | PASS | final regression | PASS mechanism |
| DATA | WAL/config lost on rollback | File copy omits WAL; early backup misses final writes | SQLite snapshot after quiescence; atomic repeated restore; newer schema rejected | WAL, schema, double restore, corrupted snapshot | PASS isolated | final regression | PASS mechanism, integration pending |
| CRYPTO | Manifest/package mismatch | Metadata not bound to artifact | Signature verification before parse, exact URL/version/build/size/hash, pinned artifact handle | Native/JS positive and corruption/wrong-key/version tests | PASS ephemeral keys | JS/final regression logs | PASS tests; production CI BLOCKED |
| HTTP | Memory growth / unsafe resume | Full-body fetch / unchecked partial responses | Streaming file, bounds, strong ETag/Range, backoff, cancellation | Actual loopback interruption, changed ETag, 429, ignored Range, stalled headers | PASS | final regression | PASS covered cases |
| BUILD | Toolchain/cache/provenance drift | Windows npm chooses adjacent Node; version stamp omits Cargo.lock | Pinned runtime preload, clean target, --locked, source ledger, lock stamp | Node enforcement, stamp tests, clean build | Native/package build PASS; runner failed on timing test, full release rerun PASS | current build evidence | IN_PROGRESS |
| LOG | Tokens in failure messages | Raw errors serialized | Typed categories; raw error text excluded | Secret canaries | PASS | final regression | PASS mechanism |

## 4. Remaining blockers

**Implementation incomplete (not attributed to environment):** concrete Windows Platform and authenticated initiating-user launch; protected-root/bootstrap/launcher installation and offline automatic recovery; metadata-to-download/UI integration; final physical-interface/DNS/WFP baseline proof; snapshot integration and recovery for each supported user context; retention/log coverage and complete error-stage reporting. Current code must not be advertised as a finished transactional updater.

**Environment/scope limits:** signed CI artifact for the current dirty source cannot be produced by public-key verification. No matching signed CI artifact was provided, and push/release are prohibited. Production private keys are not requested or read locally. Real SCM replacement, installation, OS reboot and active-system VPN-switch acceptance were not performed on the workstation, consistent with the user's prohibition. These are unverified gates, not successful tests.

## 5. Tests added

Native transaction transitions, stage corruption/traversal/extra files, real authority-file lock, ENOSPC/access-denial boundaries, next-transaction history, real child termination at eight stages, abandoned/concurrent mutex, persistent session fence, orchestration failure boundaries, rollback interruption, health identity/stability, SQLite snapshot/restore, signatures and pinned file, HTTP resume/cancellation/retry, log canaries. JS covers contract binding, packaging PE validation, release stamp and fail-closed gates.

## 6. Test results

Latest debug source regression: **194 library PASS, 1 opt-in external VLESS test ignored; 45 maintenance PASS; 35 updater PASS**. Frontend **39 PASS**. JS **18 PASS** under Node 22.23.3. Counts include overlapping modules compiled into separate native binaries; do not interpret them as that many distinct scenarios. Evidence: `temp/updater-final-regression.log`, `temp/updater-frontend-tests.log`, `temp/updater-js-tests.log`; binary hashes before cache cleanup: `temp/updater-tested-binaries.json`.

Sandbox execution initially denied atomic replacement in temporary directories. The same isolated tests passed outside sandbox; no system install/network changes were required. An HTTP test fixture initially waited indefinitely for a pooled connection to close; replaced with an explicit bounded completion channel. Both failures are preserved in earlier logs and are not counted as PASS.

## 7. Fault-injection results

Actual process termination passed at Prepared, Quiescing, Quiesced, SwitchIntent, Activated, HealthPending, RollbackPending, Committed. Before commit, reopened authority selects the complete previous version; after commit, the complete candidate. Prior version remains intact. Real Windows sharing lock prevents journal replacement without losing the prior record. ENOSPC and access-denied injection preserve previous payload. SCM/network operations in orchestration tests are controlled boundaries; they do not establish live Windows system rollback.

## 8. Package verification

`verify-package-content.cjs` checks exact extracted inventory against generated NSIS input mappings, SHA-256/size, native helper equality, actual AMD64 PE32+ headers and DLL imports. Windows PE version resources and native `--protocol` reports are recorded separately. Atlas.Service.exe is created by installer copy, not extracted as an independent package member; its intended byte identity is recorded separately from an executed installation check.

Old Phase0 artifact 2.3.2-alpha.1: 189519032 bytes, SHA-256 a0d3dfa44206eece259df2855ff930174f6740d271f99115f8ce5013822ce26b. It is retained solely as previous evidence. Current unsigned 2.4.1: 189754036 bytes, SHA-256 `f053882958ce87c00073f9330404ee9636af3b176590a4631f9498c7fe59e52d`. Clean native compilation completed in 8m50s; NSIS packaging completed. Package verifier passed for 33 files. Both native helpers ran without CEF with protocol=1/version=2.4.1/build=p0-20261007-133314. Extracted React UI rendered 12 buttons and acknowledged IPC; service/desktop identity smoke passed without starting SCM/VPN. Evidence: `temp/update-build/p0-20261007-133314/`.

## 9. Signature verification

Native minisign verification and independent Node Ed25519 verification have positive/negative tests. Ephemeral private keys exist only in generator process memory; repository fixture contains public material only. Exact manifest bytes are authenticated before JSON interpretation. Artifact is streamed through hash and signature verification and held without write/delete sharing. **Production-key artifact verification remains BLOCKED.**

## 10. Rollback verification

Generic orchestration restores previous version across activation/health failure boundaries; repeated rollback and interrupted rollback pass. Real SQLite backup/restore passes. Real SCM switch/restore and launch of the former installed version have not been exercised or wired into the CLI. U03 is not closed by the generic Host tests.

## 11. Crash recovery verification

Journal/file/process recovery passes. The machine-wide mutex alone is insufficient; durable session fencing now handles owner death. A stable launcher and boot-time recovery trigger are still absent. **A killed child process is not a Windows reboot test.**

## 12. Network recovery verification

Maintenance performs selective Atlas cleanup and a final clean-baseline check after driver teardown; a transient early TUN observation is no longer retained as a final failure. Existing network regression tests pass. No physical NIC/DNS reset or unrelated VPN manipulation was performed. Real updater under active VPN, network-generation changes and OS restart are not verified end to end.

## 13. CI-only verification

P0-C=BLOCKED for current dirty tree. CI pins Node/npm/Rust, builds with --locked and no reused Cargo target cache, tests AtlasUpdater and JS contracts, generates an artifact-bound runtime manifest, signs it using the existing CI signer, verifies both signatures through AtlasUpdater, and retains installer/signatures/manifest plus evidence. This wiring is implemented but has not been dispatched or verified with production signatures. Publication is rejected unless every readiness gate is exactly PASS and releaseReady=true. Local runner requires -Unsigned and never loads a private key. The workflow was edited but not dispatched.

## 14. Release readiness

**releaseReady=false. Not complete. Not releasable.** P0-A/P0-B for previous source are not promoted to current-source PASS. P0-C remains BLOCKED independently; other local work continues. The machine-readable readiness file is authoritative for publish gating, not a substitute for the evidence described above.

### Current build follow-up evidence

The 2.4.1 package verifier initially exposed a real provenance distinction: Tauri temporarily changes the documented `__TAURI_BUNDLE_TYPE_VAR_UNK` marker to `...NSS` while packaging. Source and extracted EXE differ in exactly those three bytes. Verification now reproduces that single bounded transformation on the source, then compares the full staged hash against extracted bytes; all other differences remain failures. Source hash: `2add61ab58197e82ded693e74ad407db37a8a0c290322329b8e541f11a60213a`; staged/extracted: `e02299a86b11c4e08ad534240c27167f91246c2b29ebebc49a8a95cfb9f6d236`. Prior-package regression also passed one-byte-corruption and extra-DLL rejection (`temp/package-contract-tests.log`). This is not a waiver of package integrity.

Initial clean runner result remains **failed** in `result.json`: a release test used a three-second window while workers from earlier tests could still occupy the shared HTTP admission gate. Isolated reproduction passed in 0.02s. Its fixture now first establishes an idle gate and asserts the slow response remains blocked when the replacement returns. The full release rerun passed: **194 library + 45 maintenance + 35 updater**, one opt-in external VLESS test ignored. Evidence: `backend-tests-recheck.log`. The original failure has not been erased or relabelled.

Verification scripts/CI and that cfg(test)-only fixture changed after the clean build started. Therefore the original full runner/source-freeze gate is **not PASS**, and P0-D is not promoted to complete. The recorded 2.4.1 installer is a diagnostic artifact tied to its captured build inputs, not a final deliverable satisfying all requests. A same-input, byte-identical second native build has not been established.

The packaged native updater rejected a manifest signed by the ephemeral test key using its embedded production trust (`native-only/wrong-trust-key.json`). This is a production-binary negative test; it does not substitute for the unavailable production-key positive test.