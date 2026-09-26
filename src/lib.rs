//! Shared library behind both binaries: `schwab` (CLI/TUI) and
//! `schwab-indexer` (egui basket builder).

pub mod accounts;
pub mod api;
pub mod auth;
pub mod basket;
pub mod blacklist;
pub mod config;
pub mod dividends;
pub mod indices;
pub mod orders;
pub mod portfolio;
pub mod pricing;
pub mod rebalance;
pub mod registry;
pub mod stream;
pub mod tui;
pub mod universe;
pub mod watchlist;
