//! gliffherdr: a herdr plugin that opens gliff remote desktops to any machine you can ssh to.
//!
//! Everything lives in this library; `src/main.rs` only calls [`cli::main`]. External commands
//! go through the [`sys::Sys`] seam so every module is testable with [`sys::fake::FakeSys`].

pub mod check;
pub mod cli;
pub mod config;
pub mod connect;
pub mod demo;
pub mod fix;
pub mod herdr;
pub mod hypr;
pub mod launch;
pub mod paths;
pub mod picker;
pub mod roster;
pub mod status;
pub mod sys;
pub mod target;
pub mod text;
