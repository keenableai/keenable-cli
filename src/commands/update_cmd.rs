use axoupdater::AxoUpdater;

use crate::daemon;
use crate::ui;
use crate::update;

pub async fn update() {
    ui::header("keenable update");

    if update::is_homebrew_install() {
        ui::error("This keenable was installed via Homebrew and cannot self-update");
        ui::hint(&format!("Run: {}", update::install_hint()));
        std::process::exit(1);
    }

    let mut updater = AxoUpdater::new_for("keenable-cli");
    // Without the explicit executable check, a binary running outside the
    // receipt's install root gets Ok(None) from run() and would falsely
    // report "up to date".
    if updater.load_receipt().is_err()
        || !updater
            .check_receipt_is_for_this_executable()
            .unwrap_or(false)
    {
        ui::error("This binary was not installed by the Keenable installer, so it cannot self-update");
        ui::hint(&format!("Reinstall with: {}", update::install_hint()));
        std::process::exit(1);
    }
    // The binary is the ground truth for the current version; the receipt
    // can lag behind it.
    if let Ok(version) = env!("CARGO_PKG_VERSION").parse() {
        let _ = updater.set_current_version(version);
    }
    // The installer's own progress output would clash with our UI; errors
    // still surface through the Err branch below.
    updater.disable_installer_output();

    let lines = ui::step("Checking for updates");
    match updater.run().await {
        Ok(Some(result)) => {
            ui::step_done_replace("Checked for updates", lines);
            // The daemon still runs the old binary; kill it so the next
            // command starts a fresh one.
            daemon::kill_daemon();
            ui::success(&format!(
                "Updated keenable v{} → v{}",
                env!("CARGO_PKG_VERSION"),
                result.new_version
            ));
        }
        Ok(None) => {
            ui::step_done_replace("Checked for updates", lines);
            ui::success(&format!(
                "keenable is up to date (v{})",
                env!("CARGO_PKG_VERSION")
            ));
        }
        Err(e) => {
            ui::step_done_replace("Checked for updates", lines);
            ui::error(&format!("Update failed: {}", e));
            ui::hint(&format!("Reinstall with: {}", update::install_hint()));
            std::process::exit(1);
        }
    }
}
