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
## Archival UI check on e9e7bdf

Job 112897483284 failed in the historical offline-upgrade scenario, whose immutable signed payload is 2.3.1-alpha.1, not the freshly built 2.4.2. Its artifact contains a successful render acknowledgement (12 buttons) and only the parent Atlas process remaining at the 25-second deadline; stderr records a CEF browser-info timeout. The assertion conflated successful render/IPC with final CEF process teardown.

The check retains its 25-second startup deadline. Only an already valid render/IPC acknowledgement permits a separate bounded 25-second normal-shutdown wait. A forced termination is still failure, helper processes must all exit, and a nonzero process exit is now explicitly rejected. The same signed archival bytes passed three isolated local runs; the fresh 2.4.2 payload also passed. The CI-only delayed exit was not reproduced locally, so the new remote run remains the confirmation for that runner-specific timing.

## Connected upgrade failures after the signed build passed

The signed build at f874270 passed packaging verification. Job 112919283601 then
reported `Invalid update identifier`. The legacy fixture extracted NSIS's
temporary `$PLUGINSDIR` into the simulated install root. That directory is not
part of a real installation. The harness now excludes it at extraction and
asserts its absence. Locally, the same signature-verified 2.2.3-alpha.47.1 archive
produced 31 application files with valid relative paths. Production path
validation remains unchanged.

Job 112922173526 progressed past that failure and reported
`User-context check must be started elevated`. The updater incorrectly treated
`TokenElevationTypeDefault` as non-administrative. Microsoft defines it as having
no linked token, which also includes unsplit administrator tokens:
https://learn.microsoft.com/en-us/windows/win32/api/winnt/ne-winnt-token_elevation_type

The updater now checks actual administrative group access and integrity. Normal
UAC launches retain the same-user, same-session shell token path. An unsplit
administrator uses `CreateRestrictedToken` with LUA_TOKEN and
DISABLE_MAX_PRIVILEGE, explicitly lowered to medium integrity. The token is
validated before process creation; the native child report independently binds
PID, SID, session, administrative access and integrity. System/service accounts
are rejected. No administrative desktop fallback is allowed.

Local evidence in `temp/installer-contract-fix/evidence`:

- `native-token-regression.log`: 202 library + 47 maintenance + 56 updater PASS.
- `elevated-token-test.log` and `.exit`: real restricted child process PASS,
  exit 0; its own token assertions verify medium integrity and no admin group.
- `user-context-result.json`: production helper UAC/IPC probe PASS, exit 0;
  child elevation type Limited, adminEnabled false, integrity 8192.

The privileged child test has a mandatory explicit invocation in PR and release
CI; it is excluded from ordinary non-admin developer unit-test runs. Its ignored
child driver is invoked explicitly by that test. Remote connected upgrade still
must pass on the newly built signed installer; these local checks do not stand
in for installation or reboot acceptance. The pinned previous artifact can only
be reused for harness/orchestration changes; this native change forces a rebuild.

## Debug test subsystem mismatch

Job 112931151002 passed 202 library, 47 maintenance and 56 updater tests, but the
separate restricted child test timed out. The test executable used the Windows
console subsystem in Debug; the shipped Release updater uses Windows GUI.
Local reproduction with the exact `update_user.rs` module and real token/process
APIs failed as a Debug console executable with NTSTATUS 0xc0000142. Rebuilding
the same Debug probe as GUI (PE subsystem 2) passed in 0.01 seconds, exit 0.
Evidence: `debug-token-test.*`, `debug-gui-token-test.*` and
`token-debug-gui-build.log` under `temp/installer-contract-fix/evidence`.

The updater's test executable now uses its production GUI subsystem in both
profiles. Token assertions, the 15-second deadline and the mandatory privileged
CI test are unchanged. Release executable subsystem/behavior is unchanged by
this correction. This isolates the known test difference; remote CI remains
required confirmation.

The installer-source gate also no longer downloads the complete binary-heavy
Git history. It uses GitHub's comparison API and builds fresh if history diverges,
the response hits the 300-file cap, or evidence is unavailable. Reuse still
requires the same pinned successful signed build and unchanged build inputs.
