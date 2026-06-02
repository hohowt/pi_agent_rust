//! Shared Pi types used by the CLI, agent, providers, and persistence layers.

#![forbid(unsafe_code)]
#![allow(
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::wildcard_imports
)]

pub mod error;
pub mod model;
pub mod provider;
pub mod provider_metadata;

pub use error::{Error, Result};
