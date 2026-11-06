//! Backend-agnostic core: domain model, image/PDF output and the scan service.
//! Front-ends depend on this crate; this crate depends on no backend or front-end.
//!
//! - [`scanner`]: domain types and the [`scanner::Backend`] / [`scanner::Scanner`] traits.
//! - [`service`]: the scan service: job store plus a single worker that owns the scanner.
//! - [`raster`], [`output`]: pure image decoding and PDF/JPEG/PNG writing.
//! - [`discovery`]: merging results from several device sources.

pub mod discovery;
pub mod output;
pub mod raster;
pub mod scanner;
pub mod service;
