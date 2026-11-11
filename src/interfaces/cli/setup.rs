//! Install-time setup: OS permissions and standing capability grants.
//! Runs inside the `desk-agent` runtime binary so macOS attributes the
//! permissions to the same executable that later dispatches tasks.
use crate::config::loader::RuntimeConfig;
use crate::infrastructure::database;
use crate::runtime::grants;

pub async fn run(revoke: bool) -> anyhow::Result<()> {
    let config = RuntimeConfig::from_env()?;
    database::adopt_legacy(&config.database_path, config.legacy_database_path.as_deref())?;
    let db = database::open(&config.database_path).await?;
    if revoke {
        let removed = grants::revoke(&db).await?;
        println!("Revoked {removed} standing capability grants ({}).", config.database_path.display());
        println!("macOS permissions are unchanged; remove them in System Settings > Privacy & Security if required.");
        return Ok(());
    }
    let count = grants::install(&db).await?;
    println!("Installed {count} standing capability grants ({}):", config.database_path.display());
    for capability in grants::installed(&db).await? {
        println!("  - {capability}");
    }
    print_permissions(true);
    Ok(())
}

#[cfg(target_os = "macos")]
fn print_permissions(request: bool) {
    let status = if request { crate::platforms::macos::permissions::request_all() } else { crate::platforms::macos::permissions::status() };
    let mark = |ok: bool| if ok { "granted" } else { "NOT granted - approve in System Settings > Privacy & Security, then restart desk-agent" };
    println!("macOS permissions:");
    println!("  Accessibility:    {}", mark(status.accessibility));
    println!("  Screen Recording: {}", mark(status.screen_recording));
    println!("  Input Monitoring: {}", mark(status.input_monitoring));
}

#[cfg(not(target_os = "macos"))]
fn print_permissions(_request: bool) {
    println!("No OS permission requests are implemented on this platform.");
}
