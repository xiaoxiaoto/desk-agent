//! One bounded key press for an already-authorized adapter operation.
//! The pair is allocated before the first effect and posting never retries.
use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant};
use crate::kernel::platform::DesktopApplication;
use crate::kernel::task::ActivationDiagnostic as Diagnostic;
use super::desktop::{self, DiagnosticResult};
use super::input::{observe_for, KeyboardMonitor, PreparedInputPair};
use super::permissions::{input_monitoring_available, trusted};

/// The only key exposed by the current atomic instruction.
pub(crate) const RETURN_KEY: u16 = 36;
const ROUTING_TIMEOUT: Duration = Duration::from_millis(500);

fn select(application: DesktopApplication) -> Result<desktop::CaptureTarget> {
    let target = desktop::window_target(application, false)?;
    desktop::frontmost(&target).map_err(|_| anyhow::anyhow!("Window not frontmost"))?;
    desktop::confirm_target_opaque(&target).map_err(|_| anyhow::anyhow!("Window not opaque"))?;
    Ok(target)
}

fn same_target(a: &desktop::CaptureTarget, b: &desktop::CaptureTarget) -> bool {
    a.pid == b.pid && a.number == b.number && a.window.as_ref().equals(b.window.as_ref())
        && desktop::bounds(a.position, a.size) == desktop::bounds(b.position, b.size)
}

pub(crate) fn preflight(application: DesktopApplication) -> DiagnosticResult<()> {
    if !trusted() { return Err(Diagnostic::AccessibilityUnavailable); }
    if !input_monitoring_available() { return Err(Diagnostic::PreflightUnavailable); }
    let target = select(application).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    // KeyboardMonitor::new is read-only; a disabled tap cannot prove routing.
    let tap = KeyboardMonitor::new(target.pid, 0).ok_or(Diagnostic::PreflightUnavailable)?;
    if !tap.enabled() { return Err(Diagnostic::PreflightUnavailable); }
    Ok(())
}

pub(crate) fn press_return(application: DesktopApplication) -> Result<()> {
    press_return_guarded(application, || Ok(()))
}

/// Run an adapter's read-only business check after allocation and immediately
/// before the final native target check and sole Return dispatch.
pub(crate) fn press_return_guarded(
    application: DesktopApplication, validate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let initial = select(application)?;
    let token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let tap = KeyboardMonitor::new(initial.pid, token).context("Input monitoring unavailable")?;
    ensure!(tap.enabled(), "Input monitoring unavailable");
    // Allocate the single pair before any effect; a failure here has no side effect.
    let press = PreparedInputPair::keyboard(RETURN_KEY, 0, None, Some(token))?;
    validate()?;
    let current = select(application)?;
    ensure!(same_target(&initial, &current) && tap.enabled(), "Key press target changed before dispatch");
    press.post();
    let deadline = Instant::now() + ROUTING_TIMEOUT;
    loop {
        ensure!(Instant::now() < deadline && tap.enabled(), "Key press outcome unknown");
        if tap.complete() { return Ok(()); }
        observe_for(Duration::from_millis(10));
    }
}
