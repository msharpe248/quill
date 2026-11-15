//! Quill's terminal UI.
//!
//! One loop, one direction: `Event -> Action -> update(State) -> view(State)`.
//! [`state`] holds the state and `update`; [`view`] draws it; [`app`] is the
//! run loop; [`terminal`] takes over the terminal; [`data`] is the store
//! worker the UI reads through. Commands live in [`command`], their keys in
//! [`keymap`], colours in [`theme`].
//!
//! This crate reads nothing from disk itself. The binary loads the config,
//! theme and key files and hands their contents in.

pub mod app;
pub mod command;
pub mod data;
pub mod demo;
pub mod keymap;
pub mod layout;
pub mod sample;
pub mod state;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod view;
