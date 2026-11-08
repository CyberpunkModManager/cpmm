//! cpmm: Cyberpunk 2077 mod manager for Linux/Proton. The core, as a library: the domain
//! modules and [`ops`], the Operations module that both adapters (the `cpmm` CLI in
//! `main.rs` and the Tauri GUI in `gui/src-tauri`) drive it through.

pub mod archive;
pub mod change;
pub mod collections;
pub mod conflicts;
pub mod downloads;
pub mod files;
pub mod fomod;
pub mod health;
pub mod import;
pub mod install;
pub mod loadorder;
pub mod manage;
pub mod modio;
pub mod nexus;
pub mod ops;
pub mod profiles;
pub mod requirements;
pub mod safety;
pub mod settings;
pub mod state;
pub mod steam;
pub mod tools;
pub mod vanilla;
pub mod verify;
pub mod vfs;
