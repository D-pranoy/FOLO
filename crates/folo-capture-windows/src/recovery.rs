//! The recovery state machine (ADR-0015 D3, ADR-0037 D13): what to do after
//! a [`Fault`], and which backend to try. It holds no clocks and no
//! operating-system objects, so it is tested with injected faults.
//!
//! - A screen that comes back (a mode change, the secure desktop, a
//!   disconnected session, a monitor unplugged for a while) is retried
//!   forever with a growing delay, and the agent is `paused` meanwhile.
//! - A backend that can't capture this screen is replaced by the next one of
//!   the order (`auto`: Desktop Duplication, Windows.Graphics.Capture, GDI).
//!   The privacy setting is never worked around by another backend.
//! - The process limit and unknown failures get a few retries first.

use std::time::Duration;

use crate::fault::Fault;

/// The three ways to capture a screen on Windows, in `auto`'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// DXGI Desktop Duplication.
    Dda,
    /// Windows.Graphics.Capture.
    Wgc,
    Gdi,
}

impl BackendKind {
    /// The name `capture.status` reports as `backend`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dda => "dda",
            Self::Wgc => "wgc",
            Self::Gdi => "gdi",
        }
    }

    /// What `--backend auto` tries, first to last.
    #[must_use]
    pub fn auto_order() -> Vec<Self> {
        vec![Self::Dda, Self::Wgc, Self::Gdi]
    }
}

/// What to do after a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Open the same backend again after `after`. `paused`: the agent can't capture meanwhile, so it holds its last picture.
    Retry { after: Duration, paused: bool },
    /// Open this backend now.
    Switch(BackendKind),
    /// Nothing is left to try.
    GiveUp(String),
}

/// The delays and counts of the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The first retry comes this soon; each next one waits twice as long.
    pub first_backoff: Duration,
    /// How long a retry waits at most, for a screen that comes back.
    pub max_backoff: Duration,
    /// How long it waits at most when the privacy setting denies capture (the user has to change it).
    pub max_privacy_backoff: Duration,
    /// Retries of the process limit before another backend is tried.
    pub limit_attempts: u32,
    /// Retries of a device loss or unknown failure before another backend is tried.
    pub failure_attempts: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            first_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(2),
            max_privacy_backoff: Duration::from_secs(5),
            limit_attempts: 3,
            failure_attempts: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Recovery {
    order: Vec<BackendKind>,
    current: usize,
    /// Faults since the last picture (or backend switch).
    streak: u32,
    policy: Policy,
}

impl Recovery {
    /// Starts with the first backend of `order`, which must not be empty.
    ///
    /// # Panics
    ///
    /// When `order` is empty: that is a bug of the caller.
    #[must_use]
    pub fn new(order: Vec<BackendKind>, policy: Policy) -> Self {
        assert!(
            !order.is_empty(),
            "a backend order needs at least one backend"
        );
        Self {
            order,
            current: 0,
            streak: 0,
            policy,
        }
    }

    /// The backend to open now.
    #[must_use]
    pub fn current(&self) -> BackendKind {
        self.order[self.current]
    }

    /// A picture arrived: the faults before it are forgotten.
    pub fn on_picture(&mut self) {
        self.streak = 0;
    }

    /// Decides what follows `fault`.
    pub fn on_fault(&mut self, fault: &Fault) -> Step {
        self.streak = self.streak.saturating_add(1);
        match fault {
            Fault::Unsupported => self.next_backend(fault),
            Fault::ProcessLimit => self.retry_or_switch(fault, self.policy.limit_attempts),
            Fault::DeviceLost | Fault::Other(_) => {
                self.retry_or_switch(fault, self.policy.failure_attempts)
            }
            Fault::PrivacyDenied => self.retry(self.policy.max_privacy_backoff, fault),
            Fault::AccessLost
            | Fault::SecureDesktop
            | Fault::SessionDisconnected
            | Fault::ScreenGone => self.retry(self.policy.max_backoff, fault),
        }
    }

    fn retry(&self, cap: Duration, fault: &Fault) -> Step {
        // 2^(streak - 1) times the first delay, without overflowing for a long streak.
        let doublings = self.streak.saturating_sub(1).min(16);
        let after = self
            .policy
            .first_backoff
            .saturating_mul(1 << doublings)
            .min(cap);
        Step::Retry {
            after,
            paused: fault.pauses(),
        }
    }

    fn retry_or_switch(&mut self, fault: &Fault, attempts: u32) -> Step {
        if self.streak <= attempts {
            self.retry(self.policy.max_backoff, fault)
        } else {
            self.next_backend(fault)
        }
    }

    fn next_backend(&mut self, fault: &Fault) -> Step {
        let Some(next) = self.order.get(self.current + 1).copied() else {
            return Step::GiveUp(format!(
                "no way of capturing this screen is left; the last one failed because {fault}"
            ));
        };
        self.current += 1;
        self.streak = 0;
        Step::Switch(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto() -> Recovery {
        Recovery::new(BackendKind::auto_order(), Policy::default())
    }

    fn retry(milliseconds: u64, paused: bool) -> Step {
        Step::Retry {
            after: Duration::from_millis(milliseconds),
            paused,
        }
    }

    #[test]
    fn auto_starts_with_desktop_duplication() {
        assert_eq!(auto().current(), BackendKind::Dda);
        let names: Vec<&str> = BackendKind::auto_order()
            .into_iter()
            .map(BackendKind::name)
            .collect();
        assert_eq!(names, ["dda", "wgc", "gdi"]);
    }

    #[test]
    fn a_lost_screen_is_retried_with_a_doubling_delay_up_to_the_cap_and_never_given_up() {
        let mut recovery = auto();
        let delays: Vec<Step> = (0..8)
            .map(|_| recovery.on_fault(&Fault::AccessLost))
            .collect();
        assert_eq!(
            delays,
            [
                retry(100, true),
                retry(200, true),
                retry(400, true),
                retry(800, true),
                retry(1600, true),
                retry(2000, true),
                retry(2000, true),
                retry(2000, true),
            ]
        );
        for _ in 0..1000 {
            assert!(matches!(
                recovery.on_fault(&Fault::SecureDesktop),
                Step::Retry { paused: true, .. }
            ));
        }
        assert_eq!(recovery.current(), BackendKind::Dda);
    }

    #[test]
    fn a_picture_starts_the_delays_over() {
        let mut recovery = auto();
        recovery.on_fault(&Fault::AccessLost);
        recovery.on_fault(&Fault::AccessLost);
        recovery.on_picture();
        assert_eq!(
            recovery.on_fault(&Fault::SessionDisconnected),
            retry(100, true)
        );
    }

    #[test]
    fn unsupported_moves_on_to_the_next_backend_until_none_is_left() {
        let mut recovery = auto();
        assert_eq!(
            recovery.on_fault(&Fault::Unsupported),
            Step::Switch(BackendKind::Wgc)
        );
        assert_eq!(recovery.current(), BackendKind::Wgc);
        assert_eq!(
            recovery.on_fault(&Fault::Unsupported),
            Step::Switch(BackendKind::Gdi)
        );
        let Step::GiveUp(why) = recovery.on_fault(&Fault::Unsupported) else {
            panic!("the last backend must give up");
        };
        assert!(why.contains("doesn't work for this screen"), "{why}");
    }

    #[test]
    fn the_process_limit_is_retried_three_times_and_then_another_backend_tried() {
        let mut recovery = auto();
        for expected in [100, 200, 400] {
            assert_eq!(
                recovery.on_fault(&Fault::ProcessLimit),
                retry(expected, true)
            );
        }
        assert_eq!(
            recovery.on_fault(&Fault::ProcessLimit),
            Step::Switch(BackendKind::Wgc)
        );
        // The new backend starts with a clean count.
        assert_eq!(recovery.on_fault(&Fault::AccessLost), retry(100, true));
    }

    #[test]
    fn unknown_failures_and_device_loss_get_five_retries() {
        for fault in [Fault::DeviceLost, Fault::Other("x".to_owned())] {
            let mut recovery = auto();
            for _ in 0..5 {
                assert!(matches!(recovery.on_fault(&fault), Step::Retry { .. }));
            }
            assert_eq!(recovery.on_fault(&fault), Step::Switch(BackendKind::Wgc));
        }
    }

    #[test]
    fn an_unknown_failure_does_not_pause_but_a_device_loss_does() {
        assert_eq!(
            auto().on_fault(&Fault::Other("x".to_owned())),
            retry(100, false)
        );
        assert_eq!(auto().on_fault(&Fault::DeviceLost), retry(100, true));
    }

    #[test]
    fn the_privacy_setting_is_waited_for_and_never_worked_around() {
        let mut recovery =
            Recovery::new(vec![BackendKind::Wgc, BackendKind::Gdi], Policy::default());
        for _ in 0..100 {
            assert!(matches!(
                recovery.on_fault(&Fault::PrivacyDenied),
                Step::Retry { paused: true, .. }
            ));
        }
        assert_eq!(recovery.current(), BackendKind::Wgc);
        assert_eq!(recovery.on_fault(&Fault::PrivacyDenied), retry(5000, true));
    }

    #[test]
    fn one_chosen_backend_gives_up_instead_of_switching() {
        let mut recovery = Recovery::new(vec![BackendKind::Dda], Policy::default());
        assert!(matches!(
            recovery.on_fault(&Fault::Unsupported),
            Step::GiveUp(_)
        ));
    }

    #[test]
    #[should_panic(expected = "at least one backend")]
    fn an_empty_order_is_a_bug() {
        let _ = Recovery::new(Vec::new(), Policy::default());
    }
}
