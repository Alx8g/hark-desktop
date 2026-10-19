//! Hark Desktop setup: a small sky card with one button that installs Hark.
//!
//! It carries the standard installer (the NSIS setup.exe that updates also use) and runs it out of sight, so the
//! install itself (files, Start menu entry, uninstall entry, notification link) is exactly the standard one. The
//! card replaces the installer's pages, nothing else.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Hark Desktop's setup only runs on Windows.");
    std::process::exit(1);
}
