// src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod gateway;
mod routing;
mod updater;

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error running RuBypass");
}
