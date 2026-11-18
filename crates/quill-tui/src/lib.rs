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

pub mod agent;
pub mod app;
pub mod command;
pub mod compose;
pub mod data;
pub mod demo;
pub mod editor;
pub mod files;
pub mod hook;
pub mod hyperlink;
pub mod input;
pub mod invite;
pub mod keymap;
pub mod layout;
pub mod outgoing;
pub mod palette;
pub mod settings;
pub mod setup;
pub mod state;
pub mod template;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod view;
pub mod vim;
pub mod when;
