//! Shared fleet config-file discovery.
//!
//! Prefer the single-file [`Search`], [`locate`], and [`locate_from`] APIs for
//! normal library ownership: each concern asks for its own config and owns its
//! own missing/error policy. [`discover_many`] is an additive inventory-style
//! optimization for callers that truly need several known config files from the
//! same starting directory in one ancestor walk.

#![forbid(unsafe_code)]

mod single;
pub use single::*;

mod batch;
pub use batch::{
    discover_many, discover_many_bounded, discover_many_from, BatchResult, BatchSearch,
};
