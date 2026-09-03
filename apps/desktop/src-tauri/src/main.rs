// Prevents an additional console window on Windows in release; a no-op here.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    autonomics_desktop_lib::run()
}
