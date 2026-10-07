# PR #57 follow-up audit — 2026-10-07

Audited head: eca2f9ed7ceeedf9e2ac70d6197340a20b819ec5; base: 79d08649e7413e7ff10ddff8f8aceb1f8261205e. This independent branch preserves #57 without changing or merging it.

## Confirmed root cause

update_user::restricted_desktop disabled administrative groups and lowered integrity but retained the elevated token default DACL. Locally that ACL granted full access only to SYSTEM and Administrators, with read/execute for the logon SID. Administrators is disabled in the child. A restricted GUI parent could start a console child, but initialization failed with 0xc0000142 or waited behind a Windows error dialog.

An isolated reproduction using the production token and installation ACL routines failed before the fix and passed after assigning SYSTEM and the original user full access in TokenDefaultDacl. Both GUI and console children remain medium integrity without enabled administrative membership. The new process regression fails without this fix and passes with it, including repeated execution.

This changes default permissions for objects created by the restricted token. Installation ACLs remain read/execute only; privileges, UAC and system policy are not relaxed. Preparation errors fail closed.

## Regression coverage

The former mandatory test inspected only the GUI child token. It did not execute a console helper, as real candidate health verification does. The replacement tests GUI and console grandchildren from a protected installation fixture, bounded waits, cleanup and two iterations. A separate unit test inspects the resulting default DACL. Error-dialog suppression is set in the actual restricted parent, since launching with a different token does not inherit the original parent error mode.

## CI evidence and limits

At the audited head the ordinary test job passed. Connected and offline upgrade jobs failed. There were no PR reviews, review comments or issue comments when inspected.

Run 37673253546, job 112970158006: candidate startup and frontend events occurred, maintenance protocol launch failed, health timed out, and the supervisor recorded RolledBack with network and migration restored. Diagnostic copied/system console children also failed to finish. The local reproduction confirms that console failure mode. The installed helper additionally returned Windows error 5: new CI evidence is required before claiming the complete CI failure is fixed.

Offline job 112970099075 tests the immutable signed 2.3.1-alpha.1 installer. UI rendered, but CEF exceeded the shutdown deadline. That historical failure is not relabeled as success.

## Verification

- Frontend: 39 passed; TypeScript/Vite build passed, existing chunk-size warning.
- Package/signature contracts: 19 passed.
- Final backend: library 202 passed, 1 opt-in live test ignored; maintenance 47 passed; updater 57 passed, 3 process-driver tests ignored in the ordinary run.
- Isolated native regression: token assertions and separately invoked elevated GUI/console test passed after the fix; process regression failed with the original token behavior.
- New PR CI: pending; local results do not substitute for installed-system acceptance.

Existing backend tests cover real loopback Mihomo/Xray traffic and restart, HTTPS subscription redirects, source isolation/persistence, preflight failure, rollback success/failure, killed transaction writers and repeated recovery. These do not prove all physical reboot, public provider, Wintun/WFP and installed source-switch scenarios. The user's installed Atlas and network settings were not changed. No merge, release or installer publication is performed.
