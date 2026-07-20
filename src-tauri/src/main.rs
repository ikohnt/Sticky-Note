// Prevents an extra console window from appearing on Windows in release builds.
// Do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sticky_notes_lib::run();
}
