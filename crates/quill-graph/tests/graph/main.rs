//! The Graph backend against a stand-in for the service on this machine.
//!
//! The answers are written the way Graph v1.0 writes them, with invented
//! people and mail. See `common` for how the tests make sure that no
//! request goes anywhere else.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod faults;
mod folders;
mod messages;
mod sending;
mod sync;
