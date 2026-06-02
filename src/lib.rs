//! Pi - AI coding agent CLI
//!
//! This library provides the core functionality for the Pi CLI tool,
//! a Rust port of pi-mono (TypeScript) with emphasis on:
//! - Reliability: No panics in normal operation
//! - Efficiency: Single binary, minimal dependencies
//!
//! ## Public API policy
//!
//! The `pi` crate is primarily the implementation crate for the `pi` CLI binary.
//! External consumers should treat non-`sdk` modules/types as **unstable**
//! and subject to change. Use [`sdk`] as the stable library-facing surface.
//!
//! Currently intended stable exports:
//! - [`Error`]
//! - [`PiResult`]
//! - [`sdk`] module

#![cfg(not(test))]
#![forbid(unsafe_code)]
// rch clippy probes without these allowances still expose broad, cross-module
// dormant surfaces in extension/session/SDK paths. The no-allow inventory is
// tracked in bd-63x3v.5.1; keep this crate-wide guard until the remaining
// subsystems are narrowed in their own patches.
#![allow(dead_code, clippy::unused_async)]
#![cfg_attr(
    test,
    allow(
        unused_variables,
        clippy::assertions_on_constants,
        clippy::match_same_arms,
        clippy::uninlined_format_args,
        clippy::missing_const_for_fn,
        clippy::collapsible_if
    )
)]
// Allow pedantic lints during early development - can tighten later
#![allow(
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::wildcard_imports
)]

// Allow in-crate tests that include integration test helpers to resolve `pi::...`
// paths the same way integration tests do.
extern crate self as pi;

#[doc(hidden)]
pub mod acp;
#[doc(hidden)]
pub mod agent;
#[doc(hidden)]
pub mod agent_cx;
#[doc(hidden)]
pub mod app;
#[doc(hidden)]
pub mod auth;
#[doc(hidden)]
pub mod autocomplete;
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod compaction;
#[doc(hidden)]
pub mod compaction_worker;
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub use pi_core::error;
#[doc(hidden)]
pub mod error_hints;
#[doc(hidden)]
pub use pi_http::http;
#[doc(hidden)]
pub mod interactive;
#[doc(hidden)]
pub mod keybindings;
#[doc(hidden)]
pub mod migrations;
#[doc(hidden)]
pub use pi_core::model;
#[doc(hidden)]
pub mod model_routing;
#[doc(hidden)]
pub mod model_selector;
#[doc(hidden)]
pub mod models;
#[doc(hidden)]
pub mod package_manager;
#[doc(hidden)]
pub mod platform;
#[doc(hidden)]
pub use pi_core::provider;
#[doc(hidden)]
pub use pi_core::provider_metadata;
#[doc(hidden)]
pub mod providers;
#[doc(hidden)]
pub mod resources;
#[doc(hidden)]
pub mod rpc;
#[doc(hidden)]
pub mod scheduler;
pub mod sdk;
#[doc(hidden)]
pub mod semantic_workspace_graph;
#[doc(hidden)]
pub mod session;
#[doc(hidden)]
pub mod session_index;
#[doc(hidden)]
pub mod session_metrics;
#[doc(hidden)]
pub mod session_picker;
#[cfg(feature = "sqlite-sessions")]
#[doc(hidden)]
pub mod session_sqlite;
#[doc(hidden)]
pub mod session_store_v2;
#[doc(hidden)]
pub use pi_http::sse;
#[doc(hidden)]
pub mod swarm_activity_ledger;
#[doc(hidden)]
pub mod terminal_images;
#[doc(hidden)]
pub mod theme;
#[doc(hidden)]
pub mod tools;
#[doc(hidden)]
pub mod tui;
#[doc(hidden)]
pub use pi_http::vcr;

pub use error::{Error, Result as PiResult};
