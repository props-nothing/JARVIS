//! Process lifecycle primitives.

pub mod discovery;
pub mod instance;

pub use discovery::{DiscoveryError, publish as publish_discovery, remove_if_owned};
pub use instance::{InstanceError, InstanceGuard, appears_unheld};
