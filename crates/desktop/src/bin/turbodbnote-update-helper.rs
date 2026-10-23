#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#[cfg(target_os = "windows")]
#[path = "../update_helper.rs"]
mod helper;
fn main() {
    #[cfg(target_os = "windows")]
    helper::run();
}
