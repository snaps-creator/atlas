//! Pure startup policy. The owner performs the returned connect decision and
//! calls `cancel` before manual connect, disconnect, or quit. While updater
//! health is pending, the owner passes `ready = false` instead of cancelling;
//! durable commit and all other readiness checks allow the startup delay to begin.
use crate::model::Startup;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchReason {
    ManualLaunch,
    ManualRestart,
    WindowsStartup,
    UpdaterRestart,
    InstallerFirstLaunch,
    RecoveryRestart,
}

impl LaunchReason {
    /// Exact flags; restart reasons take precedence over installer/autostart.
    /// Accepts either argv or an arguments-only slice. Health validation and
    /// readiness gating belong to the owner, not this argument classifier.
    pub fn from_args(args: &[String]) -> Self {
        let has = |flag: &str| args.iter().any(|arg| arg == flag);
        if has("--manual-restart") {
            Self::ManualRestart
        } else if has("--recovery-restart") {
            Self::RecoveryRestart
        } else if has("--updater-restart") {
            Self::UpdaterRestart
        } else if has("--installer-first-launch") {
            Self::InstallerFirstLaunch
        } else if has("--update-health") {
            Self::UpdaterRestart
        } else if has("--autostart") {
            Self::WindowsStartup
        } else {
            Self::ManualLaunch
        }
    }
}

/// A manual restart must not replay one-shot autostart or updater challenges.
pub fn manual_restart_args(executable: std::ffi::OsString) -> Vec<std::ffi::OsString> {
    vec![executable, "--manual-restart".into()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Policy {
    auto: bool,
    restore: bool,
    delay_seconds: u64,
}

impl From<&Startup> for Policy {
    fn from(settings: &Startup) -> Self {
        Self {
            auto: settings.auto_connect,
            restore: settings.restore_connection,
            delay_seconds: settings.delay_seconds,
        }
    }
}

#[derive(Debug)]
enum State {
    Pending {
        policy: Policy,
        ready_since: Option<Instant>,
    },
    Consumed,
    Cancelled,
}

#[derive(Debug)]
pub struct StartupCoordinator {
    state: State,
    restore_eligible: bool,
    show_window: bool,
}

impl StartupCoordinator {
    pub fn new(
        reason: LaunchReason,
        settings: &Startup,
        previous_connected: bool,
        user_disconnected: bool,
        previous_hidden: bool,
    ) -> Self {
        let restore_eligible = previous_connected && !user_disconnected;
        let policy = Policy::from(settings);
        let state = if policy.auto || (policy.restore && restore_eligible) {
            State::Pending {
                policy,
                ready_since: None,
            }
        } else {
            State::Cancelled
        };
        Self {
            state,
            restore_eligible,
            show_window: match reason {
                LaunchReason::ManualRestart
                | LaunchReason::UpdaterRestart
                | LaunchReason::RecoveryRestart => !previous_hidden,
                _ => !settings.start_in_tray,
            },
        }
    }

    /// Initial window policy only; polling never requests a window action.
    pub fn should_show_window(&self) -> bool {
        self.show_window
    }

    /// True after consumption or cancellation; the owner may stop polling.
    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Consumed | State::Cancelled)
    }

    /// Returns true once, after continuous readiness and the configured delay.
    /// Changes to auto/restore/delay restart a pending timer. Disabling the
    /// effective connection policy permanently cancels this startup attempt.
    /// launch_with_windows controls registration only and never gates this policy.
    /// The owner must keep ready false while update_health::pending() is true;
    /// pending health alone must not call cancel().
    pub fn poll(&mut self, now: Instant, ready: bool, settings: &Startup) -> bool {
        let State::Pending {
            policy,
            ready_since,
        } = &mut self.state
        else {
            return false;
        };
        let next = Policy::from(settings);
        if !next.auto && !(next.restore && self.restore_eligible) {
            self.cancel();
            return false;
        }
        if *policy != next {
            *policy = next;
            *ready_since = None;
        }
        if !ready {
            *ready_since = None;
            return false;
        }
        let since = *ready_since.get_or_insert(now);
        // Match the application's settings limit without overflowing Instant.
        let delay = Duration::from_secs(next.delay_seconds.min(300));
        if now.saturating_duration_since(since) < delay {
            return false;
        }
        self.state = State::Consumed;
        true
    }

    /// Terminal, idempotent cancellation; settings changes cannot re-arm it.
    pub fn cancel(&mut self) {
        self.state = State::Cancelled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REASONS: [LaunchReason; 6] = [
        LaunchReason::ManualLaunch,
        LaunchReason::ManualRestart,
        LaunchReason::WindowsStartup,
        LaunchReason::UpdaterRestart,
        LaunchReason::InstallerFirstLaunch,
        LaunchReason::RecoveryRestart,
    ];

    fn settings(bits: u8, delay_seconds: u64) -> Startup {
        Startup {
            launch_with_windows: bits & 1 != 0,
            auto_connect: bits & 2 != 0,
            start_in_tray: bits & 4 != 0,
            restore_connection: bits & 8 != 0,
            delay_seconds,
        }
    }

    #[test]
    fn manual_restart_cannot_replay_windows_startup_or_update_challenge() {
        let args = manual_restart_args("Atlas.exe".into())
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(args, vec!["Atlas.exe", "--manual-restart"]);
        let reason = LaunchReason::from_args(&args);
        assert_eq!(reason, LaunchReason::ManualRestart);
        let mut s = settings(0, 0);
        s.launch_with_windows = false;
        for hidden in [false, true] {
            let c = StartupCoordinator::new(reason, &s, false, false, hidden);
            assert_eq!(c.should_show_window(), !hidden);
            assert_ne!(reason, LaunchReason::WindowsStartup);
        }
    }

    #[test]
    fn finished_only_after_consumption_or_cancellation() {
        let now = Instant::now();
        let enabled = settings(2, 5);
        let disabled = settings(0, 5);
        let new = |s: &Startup| {
            StartupCoordinator::new(LaunchReason::ManualLaunch, s, false, false, false)
        };
        let mut c = new(&enabled);
        assert!(!c.is_finished());
        assert!(!c.poll(now, false, &enabled));
        assert!(!c.is_finished());
        assert!(!c.poll(now, true, &enabled));
        assert!(!c.is_finished());
        assert!(c.poll(now + Duration::from_secs(5), true, &enabled));
        assert!(c.is_finished());
        assert!(!c.poll(now + Duration::from_secs(10), true, &enabled));
        assert!(c.is_finished());
        c.cancel();
        assert!(c.is_finished());

        let mut cancelled = new(&enabled);
        cancelled.cancel();
        assert!(cancelled.is_finished());
        assert!(!cancelled.poll(now, true, &enabled));
        assert!(cancelled.is_finished());

        assert!(new(&disabled).is_finished());
        let mut changed = new(&enabled);
        assert!(!changed.poll(now, false, &disabled));
        assert!(changed.is_finished());
        assert!(!changed.poll(now, true, &enabled));
        assert!(changed.is_finished());
    }

    #[test]
    fn exhaustive_settings_history_visibility_and_launch_reasons() {
        let now = Instant::now();
        for bits in 0..16 {
            for connected in [false, true] {
                for disconnected in [false, true] {
                    for hidden in [false, true] {
                        for reason in REASONS {
                            let s = settings(bits, 2);
                            let mut c = StartupCoordinator::new(
                                reason,
                                &s,
                                connected,
                                disconnected,
                                hidden,
                            );
                            let show = if matches!(
                                reason,
                                LaunchReason::ManualRestart
                                    | LaunchReason::UpdaterRestart
                                    | LaunchReason::RecoveryRestart
                            ) {
                                !hidden
                            } else {
                                !s.start_in_tray
                            };
                            assert_eq!(c.should_show_window(), show);
                            assert!(!c.poll(now, false, &s));
                            assert!(!c.poll(now + Duration::from_secs(100), true, &s));
                            assert!(!c.poll(now + Duration::from_secs(101), true, &s));
                            assert_eq!(
                                c.poll(now + Duration::from_secs(102), true, &s),
                                s.auto_connect
                                    || (s.restore_connection && connected && !disconnected)
                            );
                            assert!(!c.poll(now + Duration::from_secs(1000), true, &s));
                            assert_eq!(c.should_show_window(), show);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn flags_are_exact_and_precedence_is_order_independent() {
        let classify = |args: &[&str]| {
            LaunchReason::from_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        for (flag, reason) in [
            ("--autostart", LaunchReason::WindowsStartup),
            ("--manual-restart", LaunchReason::ManualRestart),
            ("--updater-restart", LaunchReason::UpdaterRestart),
            ("--update-health", LaunchReason::UpdaterRestart),
            (
                "--installer-first-launch",
                LaunchReason::InstallerFirstLaunch,
            ),
            ("--recovery-restart", LaunchReason::RecoveryRestart),
        ] {
            assert_eq!(classify(&["Atlas.exe", flag]), reason);
            assert_eq!(classify(&[flag]), reason);
            assert_eq!(
                classify(&[&format!("{flag}=true")]),
                LaunchReason::ManualLaunch
            );
        }
        assert_eq!(
            classify(&[
                "--update-health",
                "transaction",
                "nonce",
                "--installer-first-launch"
            ]),
            LaunchReason::InstallerFirstLaunch
        );
        assert_eq!(classify(&[]), LaunchReason::ManualLaunch);
        assert_eq!(
            classify(&["--launch", "--unknown"]),
            LaunchReason::ManualLaunch
        );
        let flags = [
            "--autostart",
            "--installer-first-launch",
            "--updater-restart",
            "--recovery-restart",
        ];
        for i in 0..flags.len() {
            for j in i..flags.len() {
                assert_eq!(classify(&[flags[i], flags[j]]), classify(&[flags[j]]));
                assert_eq!(classify(&[flags[j], flags[i]]), classify(&[flags[j]]));
            }
        }
    }

    #[test]
    fn manual_actions_cancel_permanently_at_every_pending_stage() {
        let now = Instant::now();
        for _action in ["manual connect", "manual disconnect", "quit"] {
            for stage in 0..3 {
                let mut s = settings(2, 5);
                let mut c =
                    StartupCoordinator::new(LaunchReason::UpdaterRestart, &s, true, false, true);
                if stage >= 1 {
                    assert!(!c.poll(now, false, &s));
                }
                if stage >= 2 {
                    assert!(!c.poll(now, true, &s));
                }
                c.cancel();
                c.cancel();
                for bits in 0..16 {
                    s = settings(bits, 0);
                    assert!(!c.poll(now + Duration::from_secs(1000), true, &s));
                    assert!(!c.should_show_window());
                }
            }
        }
    }

    #[test]
    fn updater_health_gates_readiness_then_allows_one_delayed_connection() {
        let now = Instant::now();
        let reason = LaunchReason::from_args(&["--update-health".to_owned()]);
        for bits in [2, 8] {
            // auto-connect and restore-only policies
            for hidden in [false, true] {
                let s = settings(bits, 5);
                let mut c = StartupCoordinator::new(reason, &s, true, false, hidden);
                for elapsed in [0, 5, 60, 600] {
                    assert!(!c.poll(now + Duration::from_secs(elapsed), false, &s));
                    assert_eq!(c.should_show_window(), !hidden);
                }
                // The owner observes durable commit and remaining readiness.
                let ready_at = now + Duration::from_secs(601);
                assert!(!c.poll(ready_at, true, &s));
                assert!(!c.poll(ready_at + Duration::from_secs(4), true, &s));
                assert!(c.poll(ready_at + Duration::from_secs(5), true, &s));
                assert!(!c.poll(ready_at + Duration::from_secs(100), true, &s));
                assert_eq!(c.should_show_window(), !hidden);
            }
        }
    }

    #[test]
    fn main_window_starts_hidden_until_startup_policy_is_applied() {
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("Tauri configuration must be valid JSON");
        let windows = config["app"]["windows"]
            .as_array()
            .expect("Configured windows");
        let main_windows: Vec<_> = windows
            .iter()
            .filter(|window| window["label"].as_str().unwrap_or("main") == "main")
            .collect();
        assert_eq!(
            main_windows.len(),
            1,
            "Exactly one main window must be configured"
        );
        assert_eq!(
            main_windows[0]["visible"].as_bool(),
            Some(false),
            "Main must explicitly start hidden to prevent a flash before startup policy runs"
        );
    }

    #[test]
    fn all_pending_policy_transitions_reset_or_cancel() {
        let now = Instant::now();
        for connected in [false, true] {
            for disconnected in [false, true] {
                for before in 0..16 {
                    for after in 0..16 {
                        let a = settings(before, 5);
                        let b = settings(after, 5);
                        let eligible = |s: &Startup| {
                            s.auto_connect || (s.restore_connection && connected && !disconnected)
                        };
                        let mut c = StartupCoordinator::new(
                            LaunchReason::ManualLaunch,
                            &a,
                            connected,
                            disconnected,
                            false,
                        );
                        assert!(!c.poll(now, true, &a));
                        assert!(!c.poll(now + Duration::from_secs(4), true, &b));
                        let armed = eligible(&a) && eligible(&b);
                        let changed = Policy::from(&a) != Policy::from(&b);
                        assert_eq!(
                            c.poll(now + Duration::from_secs(5), true, &b),
                            armed && !changed
                        );
                        assert_eq!(
                            c.poll(now + Duration::from_secs(9), true, &b),
                            armed && changed
                        );
                        assert!(!c.poll(now + Duration::from_secs(100), true, &settings(2, 0)));
                    }
                }
            }
        }
    }

    #[test]
    fn delay_edits_restart_from_observation_and_zero_still_requires_readiness() {
        let now = Instant::now();
        for delay in [0, 1, 20, u64::MAX] {
            let mut s = settings(2, 10);
            let mut c =
                StartupCoordinator::new(LaunchReason::ManualLaunch, &s, false, false, false);
            assert!(!c.poll(now, true, &s));
            s.delay_seconds = delay;
            let changed = now + Duration::from_secs(9);
            assert!(!c.poll(changed, false, &s));
            assert_eq!(c.poll(changed, true, &s), delay == 0);
            if delay != 0 {
                assert!(!c.poll(
                    changed + Duration::from_secs(delay.min(300)) - Duration::from_nanos(1),
                    true,
                    &s
                ));
                assert!(c.poll(changed + Duration::from_secs(delay.min(300)), true, &s));
            }
            assert!(!c.poll(changed + Duration::from_secs(1000), true, &s));
        }
    }

    #[test]
    fn readiness_loss_restarts_delay_and_clock_regression_cannot_fire_early() {
        let now = Instant::now();
        let s = settings(6, 5);
        let mut c = StartupCoordinator::new(LaunchReason::ManualLaunch, &s, false, false, false);
        assert!(!c.poll(now, true, &s));
        assert!(!c.poll(now + Duration::from_secs(4), false, &s));
        assert!(!c.poll(now + Duration::from_secs(50), true, &s));
        assert!(!c.poll(now, true, &s));
        assert!(!c.poll(now + Duration::from_secs(54), true, &s));
        assert!(c.poll(now + Duration::from_secs(55), true, &s));
        assert!(!c.should_show_window());
        c.cancel();
        assert!(!c.poll(now + Duration::from_secs(100), true, &s));
    }
}
