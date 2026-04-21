use crate::config;
use crate::ui;

pub fn configure(api_key: &str) {
    ui::header("keenable configure");

    config::set_api_key(api_key);
    ui::success("API key saved");

    ui::hint("You can now use: keenable search \"query\"");
    eprintln!();
}
