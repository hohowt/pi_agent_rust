//! HTTP, SSE, and provider VCR support shared by Pi providers and auth flows.

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

pub mod http;
pub mod sse;
pub mod vcr;
