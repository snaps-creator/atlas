# Installer / CI compatibility investigation

## Failure identification

- Workflow: `Verify connected upgrade` (`connected-upgrade.yml`).
- Reusable workflow: `Build signed PR installer` (`pr-installer.yml`).
- Job: `build-installer / installer` (GitHub job 112879181216).
- Step: `Verify packaged interface and process cleanup`.
- Entry command: `./scripts/test-packaged-installer.ps1 -Installer artifacts/release/Atlas_2.4.2_x64-setup.exe`.
- Failing nested command: `test-service-identity.ps1 -Executable <extracted>/$PLUGINSDIR/payload/Atlas.exe`.
- Locally reproduced exit code: **1**.
- Exact error: `Identity acceptance requires an isolated extracted package, not an installation`.
- The identity failure is locally reproduced. The remote job subsequently failed earlier, in Build signed installer (step 10), as detailed below; it never reached the identity assertion.

## Root cause and classification

**A: the transactional installer is correct; the identity test is obsolete.**

| CI expects | Transactional installer actually contains |
|---|---|
| App in extraction root | App under `$PLUGINSDIR/payload/` (entry script already resolves it) |
| `Atlas.Service.exe` absent; test creates it | A required, receipt-bound `Atlas.Service.exe`, identical to the app |
| Test may delete its sibling service after checking | Packaged service must remain intact; tests use their own temporary copy |
| Historically one/two maintenance helper occurrences | Required maintenance role at the resolved payload path; no arbitrary recursive file count |

Verified local 2.4.1 and 2.4.2 installer inventories have **no path differences**. This layout already worked in the local transactional 2.4.1 installer. The regression is caused by routing CI to that layout without updating the older identity fixture, not by a component disappearing from 2.4.2.

The stable installed launcher is native updater bytes copied to root `Atlas.exe`; `AtlasLauncher.exe` is not a missing file. Service/desktop use the active version directory. Read-only inspection of the user's installed 2.4.2 confirmed root launcher/updater hashes equal the active updater, service ImagePath targets that version, and journal stage is Committed. No install, stop, network change or reboot was performed.

## Remote CI failure: repeated MSVC initialization

The completed job 112879181216 failed in `Build signed installer` with exit code **1**, after Tauri created its intermediate NSIS payload. Full error:

```
The input line is too long.
The syntax of the command is incorrect.
Exception: scripts/initialize-msvc.ps1:7
MSVC environment initialization failed
```

CI setup, the release builder and the transactional builder each invoke the initializer. The original implementation unconditionally re-entered VsDevCmd and did not export its initialization markers between CI steps. Re-expanding the already populated tool PATH crosses cmd.exe's 8191-character command limit. A local reproduction with a 7424-character input PATH and the same missing-marker boundary produces the same error and exit code 1.

The initializer now reuses a valid x64 compiler environment, exports VSCMD_VER/VSCMD_ARG_TGT_ARCH between CI steps and deduplicates the CMake/PATH entries. Its regression test repeats initialization, checks marker export and compiles a real C translation unit with PATH longer than 8191 characters. Both signed-build workflows run that test. No compiler/toolchain version was changed.

## Minimal fix

- Keep production signature verification; no bypass option added.
- Validate transactional required roles, app/service equality, product versions, x64 PE headers, pinned engines/geodata and full native payload receipt.
- Run the actual packaged service/app identity pair in a uniquely created copy. Require the service for transactional packages; retain legacy flat-package support.
- Resolve the maintenance role by its contract path instead of a hard-coded count.
- Share the release contract between packaging verification and CI acceptance.
- Add automatic negative regressions for missing updater/launcher, app, service, runtime and mismatched real PE version metadata. Restore and revalidate each disposable fixture.
- No installer, launcher, service or network implementation changes.

## Evidence scope

The original full entry command fails locally at the identity assertion after signature verification, native maintenance inspection and rendered UI/IPC succeed. The corrected full command passes on the same installer bytes.

For full local CI-equivalent commands the detached signature uses a fresh ephemeral test key in an isolated copy of configuration. The private key exists only in memory. Production public trust and signing identity are unchanged. This proves the same signature assertions and payload/UI/identity checks; it does not claim local production signing. The installer byte hash is unchanged by detached test signing.

Logs and mutation evidence are retained under `evidence/`. Fresh Release build and final validation results are recorded in `evidence/final-validation.json` once completed.

## Validation completed locally

- Clean Release build in a new empty Rust target: PASS.
- Transactional installer build, 32-file extraction, metadata, x64 and corruption rejection: PASS.
- Corrected full packaged CI command on freshly built installer: PASS, including real UI rendering/IPC and service identity.
- Negative component/version validation and restoration: PASS.
- Native tests from that target: 202 library + 47 maintenance + 55 updater = 304 PASS; one opt-in credential test remains intentionally ignored.
- Frontend: 39 PASS. Build/signature contracts: 19 PASS.
- Repeated/long-PATH MSVC regression: PASS.

Installer SHA-256: `e0de03c4cbb7683b344911500cb098f8f405710e06ef11ba342e6cb978386d42`.
The clean installer uses application source c38dc31; the subsequent correction changes only CI/test/build-initialization scripts and documentation. It requires a new remote CI run, not a claim that the previous failed run became green.

## PR handling

The user initially required isolation; no PR changes were made during reproduction. The later explicit instruction authorizes applying these verified CI fixes directly to PR #57. No merge, release publication or host installation is performed.