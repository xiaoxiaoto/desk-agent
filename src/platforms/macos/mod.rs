pub(crate) mod process;
pub(crate) mod permissions;
pub(crate) mod desktop;
pub(crate) mod input;
pub(crate) mod field;
pub(crate) mod keyboard;
pub(crate) mod screenshot;

use crate::kernel::platform::LaunchTarget;

/// Launch the application selected by the authorized application adapter.
pub(crate) fn launch(target: LaunchTarget) -> anyhow::Result<()> {
    let status = std::process::Command::new("open")
        .args(["-a", target.application_name])
        .status()?;
    anyhow::ensure!(status.success(), "macOS open command failed");
    Ok(())
}
