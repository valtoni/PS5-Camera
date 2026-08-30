use crate::CancellationSignal;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar, Mutex, MutexGuard,
};
use std::time::Duration;
use thiserror::Error;

pub const WINDOWS_SERVICE_NAME: &str = "PS5CameraService";
pub const SCM_DEFAULT_WAIT_HINT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScmPhase {
    StartPending,
    Running,
    StopPending,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScmEventKind {
    Running,
    ReadinessBlocked,
    StopRequested,
    StopAlreadyPending,
    Interrogated,
    ControlIgnored,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScmBlocker {
    FirmwareUnavailable,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScmLogRecord {
    pub sequence: u64,
    pub phase: ScmPhase,
    pub event: ScmEventKind,
    pub control_code: Option<u32>,
    pub blocker: Option<ScmBlocker>,
    pub win32_exit_code: u32,
    pub service_specific_exit_code: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScmStatusSnapshot {
    pub phase: ScmPhase,
    pub accepts_stop: bool,
    pub accepts_shutdown: bool,
    pub win32_exit_code: u32,
    pub service_specific_exit_code: u32,
    pub checkpoint: u32,
    pub wait_hint: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmControl {
    Stop,
    Shutdown,
    Interrogate,
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmControlAction {
    RequestStop,
    ReportStatus,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid SCM transition from {from:?} to {to:?}")]
pub struct ScmTransitionError {
    pub from: ScmPhase,
    pub to: ScmPhase,
}

/// Pure SCM lifecycle. Windows API calls consume its snapshots but do not own
/// transition policy, keeping status behavior deterministic in tests.
#[derive(Debug, Clone)]
pub struct ScmLifecycle {
    phase: ScmPhase,
    sequence: u64,
    win32_exit_code: u32,
    service_specific_exit_code: u32,
}

impl Default for ScmLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl ScmLifecycle {
    pub const fn new() -> Self {
        Self {
            phase: ScmPhase::StartPending,
            sequence: 0,
            win32_exit_code: 0,
            service_specific_exit_code: 0,
        }
    }

    pub const fn phase(&self) -> ScmPhase {
        self.phase
    }

    pub fn status(&self) -> ScmStatusSnapshot {
        let (accepts_controls, checkpoint, wait_hint) = match self.phase {
            ScmPhase::StartPending => (false, 1, SCM_DEFAULT_WAIT_HINT),
            ScmPhase::Running => (true, 0, Duration::ZERO),
            ScmPhase::StopPending => (false, 2, SCM_DEFAULT_WAIT_HINT),
            ScmPhase::Stopped => (false, 0, Duration::ZERO),
        };
        ScmStatusSnapshot {
            phase: self.phase,
            accepts_stop: accepts_controls,
            accepts_shutdown: accepts_controls,
            win32_exit_code: self.win32_exit_code,
            service_specific_exit_code: self.service_specific_exit_code,
            checkpoint,
            wait_hint,
        }
    }

    pub fn mark_running(&mut self) -> Result<ScmLogRecord, ScmTransitionError> {
        if self.phase != ScmPhase::StartPending {
            return Err(ScmTransitionError {
                from: self.phase,
                to: ScmPhase::Running,
            });
        }
        self.phase = ScmPhase::Running;
        Ok(self.record(ScmEventKind::Running, None, None))
    }

    pub fn note_readiness_blocked(&mut self, blocker: ScmBlocker) -> ScmLogRecord {
        self.record(ScmEventKind::ReadinessBlocked, None, Some(blocker))
    }

    pub fn handle_control(&mut self, control: ScmControl) -> (ScmControlAction, ScmLogRecord) {
        match control {
            ScmControl::Stop | ScmControl::Shutdown => {
                let raw = if control == ScmControl::Stop { 1 } else { 5 };
                if matches!(self.phase, ScmPhase::StartPending | ScmPhase::Running) {
                    self.phase = ScmPhase::StopPending;
                    (
                        ScmControlAction::RequestStop,
                        self.record(ScmEventKind::StopRequested, Some(raw), None),
                    )
                } else {
                    (
                        ScmControlAction::ReportStatus,
                        self.record(ScmEventKind::StopAlreadyPending, Some(raw), None),
                    )
                }
            }
            ScmControl::Interrogate => (
                ScmControlAction::ReportStatus,
                self.record(ScmEventKind::Interrogated, Some(4), None),
            ),
            ScmControl::Other(raw) => (
                ScmControlAction::Ignore,
                self.record(ScmEventKind::ControlIgnored, Some(raw), None),
            ),
        }
    }

    pub fn mark_stopped(
        &mut self,
        win32_exit_code: u32,
        service_specific_exit_code: u32,
    ) -> Result<ScmLogRecord, ScmTransitionError> {
        if !matches!(
            self.phase,
            ScmPhase::StartPending | ScmPhase::Running | ScmPhase::StopPending
        ) {
            return Err(ScmTransitionError {
                from: self.phase,
                to: ScmPhase::Stopped,
            });
        }
        self.phase = ScmPhase::Stopped;
        self.win32_exit_code = win32_exit_code;
        self.service_specific_exit_code = service_specific_exit_code;
        Ok(self.record(ScmEventKind::Stopped, None, None))
    }

    fn record(
        &mut self,
        event: ScmEventKind,
        control_code: Option<u32>,
        blocker: Option<ScmBlocker>,
    ) -> ScmLogRecord {
        self.sequence = self.sequence.saturating_add(1);
        ScmLogRecord {
            sequence: self.sequence,
            phase: self.phase,
            event,
            control_code,
            blocker,
            win32_exit_code: self.win32_exit_code,
            service_specific_exit_code: self.service_specific_exit_code,
        }
    }
}

/// Cooperative, idempotent stop signal shared by the SCM handler and service
/// core. Waiting uses a condition variable rather than polling.
#[derive(Debug, Default)]
pub struct ScmStopSignal {
    requested: AtomicBool,
    mutex: Mutex<()>,
    wake: Condvar,
}

impl ScmStopSignal {
    pub const fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            mutex: Mutex::new(()),
            wake: Condvar::new(),
        }
    }

    pub fn request_stop(&self) -> bool {
        let was_requested = self.requested.swap(true, Ordering::AcqRel);
        self.wake.notify_all();
        !was_requested
    }

    pub fn wait(&self) {
        let mut guard = lock_without_poison(&self.mutex);
        while !self.is_cancelled() {
            guard = match self.wake.wait(guard) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }

    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        let guard = lock_without_poison(&self.mutex);
        let _ = match self
            .wake
            .wait_timeout_while(guard, timeout, |_| !self.is_cancelled())
        {
            Ok(result) => result,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.is_cancelled()
    }
}

impl CancellationSignal for ScmStopSignal {
    fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

fn lock_without_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ScmDispatchError {
    #[error("Windows SCM hosting is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("failed to connect service dispatcher: Win32 error {code}")]
    Dispatcher { code: u32 },
    #[error("service main failed: Win32 error {code}")]
    ServiceMain { code: u32 },
}

#[cfg(not(windows))]
pub fn run_windows_service_dispatcher() -> Result<(), ScmDispatchError> {
    Err(ScmDispatchError::UnsupportedPlatform)
}

#[cfg(windows)]
pub fn run_windows_service_dispatcher() -> Result<(), ScmDispatchError> {
    crate::windows_scm::run_dispatcher()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn lifecycle_emits_deterministic_status_sequence() {
        let mut lifecycle = ScmLifecycle::new();
        assert_eq!(lifecycle.status().phase, ScmPhase::StartPending);
        assert_eq!(lifecycle.status().checkpoint, 1);

        let running = lifecycle.mark_running().unwrap();
        assert_eq!(running.sequence, 1);
        assert_eq!(running.event, ScmEventKind::Running);
        assert!(lifecycle.status().accepts_stop);

        let (action, stopping) = lifecycle.handle_control(ScmControl::Stop);
        assert_eq!(action, ScmControlAction::RequestStop);
        assert_eq!(stopping.sequence, 2);
        assert_eq!(lifecycle.status().phase, ScmPhase::StopPending);
        assert!(!lifecycle.status().accepts_stop);

        let stopped = lifecycle.mark_stopped(0, 0).unwrap();
        assert_eq!(stopped.sequence, 3);
        assert_eq!(stopped.phase, ScmPhase::Stopped);
        assert_eq!(lifecycle.status().checkpoint, 0);
    }

    #[test]
    fn duplicate_stop_is_idempotent_and_interrogate_preserves_state() {
        let mut lifecycle = ScmLifecycle::new();
        lifecycle.mark_running().unwrap();
        lifecycle.handle_control(ScmControl::Shutdown);
        let (action, duplicate) = lifecycle.handle_control(ScmControl::Stop);
        assert_eq!(action, ScmControlAction::ReportStatus);
        assert_eq!(duplicate.event, ScmEventKind::StopAlreadyPending);
        assert_eq!(lifecycle.phase(), ScmPhase::StopPending);

        let (action, interrogated) = lifecycle.handle_control(ScmControl::Interrogate);
        assert_eq!(action, ScmControlAction::ReportStatus);
        assert_eq!(interrogated.event, ScmEventKind::Interrogated);
        assert_eq!(lifecycle.phase(), ScmPhase::StopPending);
    }

    #[test]
    fn invalid_transitions_return_errors_instead_of_panicking() {
        let mut lifecycle = ScmLifecycle::new();
        lifecycle.mark_running().unwrap();
        assert_eq!(
            lifecycle.mark_running(),
            Err(ScmTransitionError {
                from: ScmPhase::Running,
                to: ScmPhase::Running,
            })
        );
        lifecycle.mark_stopped(1066, 7).unwrap();
        assert_eq!(lifecycle.status().win32_exit_code, 1066);
        assert_eq!(lifecycle.status().service_specific_exit_code, 7);
    }

    #[test]
    fn stop_signal_wakes_waiter_once_and_is_a_cancellation_signal() {
        let signal = Arc::new(ScmStopSignal::new());
        let waiter = Arc::clone(&signal);
        let thread = thread::spawn(move || waiter.wait_timeout(Duration::from_secs(2)));
        assert!(signal.request_stop());
        assert!(!signal.request_stop());
        assert!(thread.join().unwrap());
        assert!(signal.is_cancelled());
    }

    #[test]
    fn timeout_does_not_cancel_signal() {
        let signal = ScmStopSignal::new();
        assert!(!signal.wait_timeout(Duration::from_millis(1)));
        assert!(!signal.is_cancelled());
    }

    #[test]
    fn records_are_json_serializable() {
        let mut lifecycle = ScmLifecycle::new();
        let record = lifecycle.note_readiness_blocked(ScmBlocker::FirmwareUnavailable);
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("readiness_blocked"));
        assert!(json.contains("firmware_unavailable"));
        assert!(!json.contains(WINDOWS_SERVICE_NAME));
    }
}
