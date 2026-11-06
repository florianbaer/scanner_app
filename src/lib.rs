//! Core library for scanner_app. Front-ends (CLI, web) only use this public API.
//!
//! Layers:
//! - [`scanner`]: domain types and the [`scanner::Scanner`] trait.
//! - [`raster`], [`output`]: pure image/PDF handling.
//! - [`sane`], [`discovery`]: infrastructure adapters (SANE FFI, mDNS).

pub mod discovery;
pub mod output;
pub mod raster;
pub mod sane;
pub mod scanner;
