//! Shared library behind both binaries: `schwab` (CLI/TUI) and
//! `schwab-indexer` (egui basket builder).

pub mod accounts;
pub mod alphavantage;
pub mod api;
pub mod auth;
pub mod backtest;
pub mod basket;
pub mod blacklist;
pub mod bonds;
pub mod config;
pub mod dividends;
pub mod execution;
pub mod factors;
pub mod history;
pub mod indices;
pub mod orders;
pub mod portfolio;
pub mod pricing;
pub mod rebalance;
pub mod risk;
pub mod registry;
pub mod stream;
pub mod tui;
pub mod universe;
pub mod watchlist;
