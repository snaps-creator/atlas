This branch contains a disposable diagnosis, not a change to PR #60 or release readiness.

Dispatch the existing release workflow on `audit/tray-root-cause` with confirmation `Diagnostic only - do not publish`.
All publishing jobs require main and the exact release confirmation, so they remain skipped.
Only this reusable diagnostic job runs. It builds a small fixture, not Atlas or an installer.

The fixture includes the unmodified production update_user module. It creates its own tray window/icon,
then launches the same executable with the exact production limited token. It records the native
NIM_ADD Boolean/GetLastError and Shell_NotifyIconGetRect result in both contexts, including the parent's
query of the still-live child. It removes only its own icon and exits. No VPN/service/installation,
existing Explorer termination, registry changes, security filter changes or publication occurs.

The digest helper deliberately rejects unrelated installer-probe operations; it is unused here.
