//! Keep a sent profile-install request alive while an already-connected phone
//! is unresponsive. A timeout is not evidence that the device is locked.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

pub const CANCELLED: &str = "Renewal wait canceled by user.";

#[derive(Default)]
pub struct Control {
    waiting: AtomicBool,
    canceled: AtomicBool,
    wake: Notify,
}

impl Control {
    pub fn reset(&self) {
        self.waiting.store(false, Ordering::SeqCst);
        self.canceled.store(false, Ordering::SeqCst);
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting.load(Ordering::SeqCst)
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    async fn canceled(&self) {
        loop {
            let notified = self.wake.notified();
            if self.is_canceled() {
                return;
            }
            notified.await;
        }
    }
}

struct Waiting<'a>(&'a Control);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.waiting.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Probe {
    Installed,
    /// An authenticated profile listing confirms the UUID is not installed.
    Connected,
    /// Endpoint reachable, but profile installation status could not be read.
    Unresponsive,
    Disconnected(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Completion<T = ()> {
    Acknowledged(T),
    Verified,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttemptError {
    Retryable(String),
    Fatal(String),
}

pub enum WaitReason {
    NoResponse,
    SocketTimeout(String),
}

#[derive(Clone, Copy)]
pub struct Policy {
    pub grace: Duration,
    pub poll: Duration,
    pub max_wait: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(60),
            poll: Duration::from_secs(15),
            max_wait: Duration::from_secs(12 * 60 * 60),
        }
    }
}

/// Keep a slow request alive. If its transport ends with a retryable timeout,
/// verify the exact profile UUID before starting another request. Every retry
/// must own a fresh connection; a timed-out stream must never be reused.
pub async fn await_install<A, I, P, F, T>(
    mut attempt: A,
    mut probe: P,
    control: &Control,
    policy: Policy,
    mut on_wait: impl FnMut(WaitReason),
) -> Result<Completion<T>, String>
where
    A: FnMut() -> I,
    I: Future<Output = Result<T, AttemptError>>,
    P: FnMut() -> F,
    F: Future<Output = Probe>,
{
    let mut active = Some(Box::pin(attempt()));
    let first = tokio::select! {
        biased;
        _ = control.canceled() => return Err(CANCELLED.into()),
        result = active.as_mut().unwrap() => Some(result),
        _ = tokio::time::sleep(policy.grace) => None,
    };
    let reason = match first {
        Some(Ok(value)) => return Ok(Completion::Acknowledged(value)),
        Some(Err(AttemptError::Fatal(error))) => return Err(error),
        Some(Err(AttemptError::Retryable(error))) => {
            active = None;
            WaitReason::SocketTimeout(error)
        }
        None => WaitReason::NoResponse,
    };

    control.waiting.store(true, Ordering::SeqCst);
    let _waiting = Waiting(control);
    on_wait(reason);
    // This deadline belongs to the whole wait, not to each reconnect attempt.
    let deadline = tokio::time::sleep(policy.max_wait);
    tokio::pin!(deadline);
    let expired = || {
        format!(
            "iPhone response wait expired after {} seconds.",
            policy.max_wait.as_secs()
        )
    };

    loop {
        let check = probe();
        tokio::pin!(check);
        let status = loop {
            tokio::select! {
                biased;
                _ = control.canceled() => return Err(CANCELLED.into()),
                _ = &mut deadline => return Err(expired()),
                result = async { active.as_mut().unwrap().as_mut().await }, if active.is_some() => {
                    match result {
                        Ok(value) => return Ok(Completion::Acknowledged(value)),
                        Err(AttemptError::Fatal(error)) => return Err(error),
                        Err(AttemptError::Retryable(_)) => active = None,
                    }
                }
                status = &mut check => break status,
            }
        };
        match status {
            Probe::Installed => return Ok(Completion::Verified),
            Probe::Disconnected(error) => {
                return Err(format!("iPhone disconnected while waiting: {error}"))
            }
            Probe::Connected if active.is_none() => active = Some(Box::pin(attempt())),
            Probe::Connected | Probe::Unresponsive => {}
        }
        let pause = tokio::time::sleep(policy.poll);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                biased;
                _ = control.canceled() => return Err(CANCELLED.into()),
                _ = &mut deadline => return Err(expired()),
                result = async { active.as_mut().unwrap().as_mut().await }, if active.is_some() => {
                    match result {
                        Ok(value) => return Ok(Completion::Acknowledged(value)),
                        Err(AttemptError::Fatal(error)) => return Err(error),
                        Err(AttemptError::Retryable(_)) => active = None,
                    }
                }
                _ = &mut pause => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::future::{pending, ready};

    fn policy() -> Policy {
        Policy {
            grace: Duration::from_secs(60),
            poll: Duration::from_secs(15),
            max_wait: Duration::from_secs(3600),
        }
    }

    fn timeout() -> AttemptError {
        AttemptError::Retryable("Socket(Kind(TimedOut))".into())
    }

    #[tokio::test(start_paused = true)]
    async fn socket_timeout_at_22_seconds_checks_installation_before_resending() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = await_install(
            || {
                attempts.set(attempts.get() + 1);
                async {
                    tokio::time::sleep(Duration::from_secs(22)).await;
                    Err::<(), _>(timeout())
                }
            },
            || ready(Probe::Installed),
            &control,
            policy(),
            |reason| assert!(matches!(reason, WaitReason::SocketTimeout(_))),
        )
        .await;
        assert_eq!(result, Ok(Completion::Verified));
        assert_eq!(attempts.get(), 1);
        assert_eq!(started.elapsed(), Duration::from_secs(22));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn missing_profile_is_retried_only_after_successful_verification() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let checked = Cell::new(false);
        let result = await_install(
            || {
                let n = attempts.get() + 1;
                attempts.set(n);
                if n > 1 {
                    assert!(checked.get());
                }
                ready(if n == 1 {
                    Err(timeout())
                } else {
                    Ok("fresh connection")
                })
            },
            || {
                checked.set(true);
                ready(Probe::Connected)
            },
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert_eq!(result, Ok(Completion::Acknowledged("fresh connection")));
        assert_eq!(attempts.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_installation_status_never_resends_the_profile() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let result = await_install(
            || {
                attempts.set(attempts.get() + 1);
                ready(Err::<(), _>(timeout()))
            },
            || ready(Probe::Unresponsive),
            &control,
            Policy {
                max_wait: Duration::from_secs(50),
                ..policy()
            },
            |_| {},
        )
        .await;
        assert!(result.unwrap_err().contains("wait expired"));
        assert_eq!(attempts.get(), 1);
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn service_recovery_allows_retry_after_unknown_status() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let probes = Cell::new(0);
        let result = await_install(
            || {
                let n = attempts.get() + 1;
                attempts.set(n);
                if n > 1 {
                    assert!(probes.get() >= 2);
                }
                ready(if n == 1 { Err(timeout()) } else { Ok(()) })
            },
            || {
                probes.set(probes.get() + 1);
                ready(if probes.get() == 1 {
                    Probe::Unresponsive
                } else {
                    Probe::Connected
                })
            },
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert_eq!(result, Ok(Completion::Acknowledged(())));
        assert_eq!(attempts.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_socket_timeouts_do_not_reset_the_wait_deadline_or_spin() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = await_install(
            || {
                attempts.set(attempts.get() + 1);
                ready(Err::<(), _>(timeout()))
            },
            || ready(Probe::Connected),
            &control,
            Policy {
                max_wait: Duration::from_secs(50),
                ..policy()
            },
            |_| {},
        )
        .await;
        assert!(result.unwrap_err().contains("wait expired"));
        assert_eq!(started.elapsed(), Duration::from_secs(50));
        assert_eq!(attempts.get(), 5); // first request, then at 0/15/30/45 seconds
    }

    #[tokio::test(start_paused = true)]
    async fn socket_timeout_after_the_grace_period_can_also_recover() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let result = await_install(
            || {
                let n = attempts.get() + 1;
                attempts.set(n);
                async move {
                    if n == 1 {
                        tokio::time::sleep(Duration::from_secs(65)).await;
                        Err(timeout())
                    } else {
                        Ok(())
                    }
                }
            },
            || ready(Probe::Connected),
            &control,
            policy(),
            |reason| assert!(matches!(reason, WaitReason::NoResponse)),
        )
        .await;
        assert_eq!(result, Ok(Completion::Acknowledged(())));
        assert_eq!(attempts.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn disconnected_phone_is_not_retried_inside_the_wait() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let result = await_install(
            || {
                attempts.set(attempts.get() + 1);
                ready(Err::<(), _>(timeout()))
            },
            || ready(Probe::Disconnected("offline".into())),
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert!(result.unwrap_err().contains("disconnected"));
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_rejection_on_a_new_connection_ends_the_wait() {
        let control = Control::default();
        let attempts = Cell::new(0);
        let result = await_install(
            || {
                attempts.set(attempts.get() + 1);
                ready(Err::<(), _>(if attempts.get() == 1 {
                    timeout()
                } else {
                    AttemptError::Fatal("MisagentFailure".into())
                }))
            },
            || ready(Probe::Connected),
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert_eq!(result, Err("MisagentFailure".into()));
        assert_eq!(attempts.get(), 2);
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn early_socket_timeout_wait_can_be_canceled() {
        let control = Control::default();
        let wait = await_install(
            || ready(Err::<(), _>(timeout())),
            || pending::<Probe>(),
            &control,
            policy(),
            |_| {},
        );
        let cancel = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert!(control.is_waiting());
            control.cancel();
        };
        let (result, ()) = tokio::join!(wait, cancel);
        assert_eq!(result, Err(CANCELLED.into()));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_success_never_enters_waiting() {
        let control = Control::default();
        let result = await_install(
            || ready(Ok(())),
            || async { panic!("must not probe") },
            &control,
            policy(),
            |_| panic!("must not wait"),
        )
        .await;
        assert_eq!(result, Ok(Completion::Acknowledged(())));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn original_request_resumes_after_grace_without_resubmitting() {
        let control = Control::default();
        let polls = Cell::new(0);
        let install = || async {
            tokio::time::sleep(Duration::from_secs(95)).await;
            Ok(())
        };
        let result = await_install(
            install,
            || {
                assert!(control.is_waiting());
                polls.set(polls.get() + 1);
                ready(Probe::Connected)
            },
            &control,
            policy(),
            |_| assert!(control.is_waiting()),
        )
        .await;
        assert_eq!(result, Ok(Completion::Acknowledged(())));
        assert!(polls.get() >= 2);
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn installed_profile_verification_recovers_missing_ack() {
        let control = Control::default();
        let result = await_install(
            || pending::<Result<(), AttemptError>>(),
            || ready(Probe::Installed),
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert_eq!(result, Ok(Completion::Verified));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn losing_connection_stops_the_wait() {
        let control = Control::default();
        let result = await_install(
            || pending::<Result<(), AttemptError>>(),
            || ready(Probe::Disconnected("offline".into())),
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert!(result.unwrap_err().contains("disconnected"));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_interrupts_a_stalled_probe() {
        let control = Control::default();
        let wait = await_install(
            || pending::<Result<(), AttemptError>>(),
            || pending::<Probe>(),
            &control,
            policy(),
            |_| {},
        );
        let cancel = async {
            tokio::time::sleep(Duration::from_secs(61)).await;
            assert!(control.is_waiting());
            control.cancel();
        };
        let (result, ()) = tokio::join!(wait, cancel);
        assert_eq!(result, Err(CANCELLED.into()));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn acknowledgment_is_processed_during_a_stalled_probe() {
        let control = Control::default();
        let install = || async {
            tokio::time::sleep(Duration::from_secs(61)).await;
            Ok(())
        };
        let result =
            await_install(install, || pending::<Probe>(), &control, policy(), |_| {}).await;
        assert_eq!(result, Ok(Completion::Acknowledged(())));
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_install_error_does_not_enter_waiting() {
        let control = Control::default();
        let result = await_install(
            || ready(Err::<(), _>(AttemptError::Fatal("rejected".into()))),
            || ready(Probe::Connected),
            &control,
            policy(),
            |_| panic!("must not wait"),
        )
        .await;
        assert_eq!(result, Err("rejected".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn connected_but_unresponsive_wait_has_a_deadline() {
        let control = Control::default();
        let result = await_install(
            || pending::<Result<(), AttemptError>>(),
            || ready(Probe::Connected),
            &control,
            policy(),
            |_| {},
        )
        .await;
        assert!(result.unwrap_err().contains("wait expired"));
        assert!(!control.is_waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn canceled_control_can_be_reused_for_a_later_job() {
        let control = Control::default();
        control.cancel();
        control.reset();
        let install = || async {
            tokio::time::sleep(Duration::from_secs(61)).await;
            Ok(())
        };
        assert_eq!(
            await_install(
                install,
                || ready(Probe::Connected),
                &control,
                policy(),
                |_| {}
            )
            .await,
            Ok(Completion::Acknowledged(()))
        );
    }
}
