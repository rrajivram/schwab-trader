//! Shared library behind both binaries: `schwab` (CLI/TUI) and
//! `schwab-indexer` (egui basket builder).

pub mod accounts;
pub mod api;
pub mod auth;
pub mod blacklist;
pub mod config;
pub mod indices;
pub mod orders;
pub mod pricing;
pub mod rebalance;
pub mod registry;
pub mod stream;
pub mod tui;
pub mod watchlist;
