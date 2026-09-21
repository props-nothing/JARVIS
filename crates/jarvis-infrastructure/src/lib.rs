//! Infrastructure adapters that implement JARVIS domain ports.
//!
//! This crate binds domain ports to operating-system facilities. It is the only
//! place allowed to know that a real clock or UUID generator exists.
#![forbid(unsafe_code)]

pub mod auth;
pub mod client;
pub mod config;
pub mod daemon;
pub mod diagnostics;
pub mod error;
pub mod http;
pub mod ids;
pub mod lifecycle;
pub mod paths;
pub mod profile;
pub mod release;
pub mod service;
pub mod storage;
pub mod time;
