//! The EWS backend against a stand-in for an Exchange server on this
//! machine.
//!
//! The answers are written by hand the way Exchange writes them, with
//! invented people and mail; no real server was recorded. See `common` for
//! how the tests make sure that no request goes anywhere else.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod faults;
mod folders;
mod messages;
mod sending;
mod sync;
