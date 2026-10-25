// Keep release builds from opening an extra console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    link2linux_desktop_lib::run();
}
