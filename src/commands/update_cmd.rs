use axoupdater::AxoUpdater;

use crate::daemon;
use crate::ui;
use crate::update;

fn bail_reinstall(msg: &str) -> ! {
    ui::error(msg);
    ui::hint(&format!("Reinstall with: {}", update::install_hint()));
    std::process::exit(1);
}

pub async fn update() {
    ui::header("keenable update");

    if update::is_homebrew_install() {
        ui::error("This keenable was installed via Homebrew and cannot self-update");
        ui::hint(&format!("Run: {}", update::install_hint()));
        std::process::exit(1);
    }

    // The receipt name must equal the cargo-dist app name, i.e. the package name.
    let mut updater = AxoUpdater::new_for(env!("CARGO_PKG_NAME"));
    // Without the explicit executable check, a binary running outside the
    // receipt's install root gets Ok(None) from run() and would falsely
    // report "up to date".
    if updater.load_receipt().is_err()
        || !updater
            .check_receipt_is_for_this_executable()
            .unwrap_or(false)
    {
        bail_reinstall(
            "This binary was not installed by the Keenable installer, so it cannot self-update",
        );
    }
    // The binary is the ground truth for the current version; the receipt
    // can lag behind it.
    if let Ok(version) = update::current_version().parse() {
        let _ = updater.set_current_version(version);
    }
    // The installer's own progress output would clash with our UI; errors
    // still surface through the Err branch below.
    updater.disable_installer_output();

    let lines = ui::step("Checking for updates");
    let outcome = updater.run().await;
    ui::step_done_replace("Checked for updates", lines);
    match outcome {
        Ok(Some(result)) => {
            // The daemon still runs the old binary; kill it so the next
            // command starts a fresh one.
            daemon::kill_daemon();
            ui::success(&format!(
                "Updated keenable v{} → v{}",
                update::current_version(),
                result.new_version
            ));
        }
        Ok(None) => {
            ui::success(&format!(
                "keenable is up to date (v{})",
                update::current_version()
            ));
        }
        Err(e) => bail_reinstall(&format!("Update failed: {}", e)),
    }
}
