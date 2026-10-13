# Atlas 2.4.3 — pre-merge remediation

Candidate, not a published release. Evidence from 2.4.2 is historical and does not certify this candidate.

## Scope

Unified URL/VLESS sources and startup lifecycle retain the reconciliation and network recovery contracts. Remediation protects node references from stale UI saves, separates runtime visibility persistence from user backup history, refuses configuration rollback when required credentials were deleted, removes installation-owned user autostart during uninstall, and gives manual restart fresh arguments.

The yellow Updates navigation button is the only requested color change. No speculative DNS, MTU, IPv6, routing or transport changes are included.

## Required acceptance

The disposable Windows runner must install the candidate, upgrade signed 2.4.2 with mixed sources and persisted references, exercise startup/restore/tray behavior and uninstall. Unit or isolated UI smoke tests do not replace installed acceptance. No READY verdict until current-SHA installed evidence passes.

## Formatting and warnings

The base branch is not globally Prettier-clean. Keep new code consistent and check scoped changes; do not reformat unrelated legacy files. A whole-project formatting failure inherited from the base is not a release blocker for this remediation. Vite's large bundle warning is retained as non-blocking: there is no demonstrated functional regression warranting a frontend splitting refactor here. Native updater APIs shared with the desktop remain available with targeted dead-code annotations.
