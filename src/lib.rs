//! vela — a Spotlight-style launcher for Hyprland.
//!
//! The pure logic (desktop entries, search, config, launching) lives in
//! modules without GTK so it can be tested and used by the lean CLI client;
//! `ui` contains everything GTK.

pub mod apps;
pub mod claude_usage;
pub mod components;
pub mod config;
pub mod history;
pub mod hypranim;
pub mod hyprconf;
pub mod hyprextra;
pub mod hyprland;
pub mod hyprmon;
pub mod idle;
pub mod ipc;
pub mod launch;
pub mod launcher_layout;
pub mod mcp;
pub mod nixos;
pub mod paths;
pub mod pulse;
pub mod quickshare;
pub mod search;
pub mod search_terms;
pub mod selfupdate;
pub mod sharepick;
pub mod shell;
pub mod theme;
pub mod ui;
pub mod update;
pub mod xdph;
