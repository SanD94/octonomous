//! UI-independent protocol core for octonomous.

pub mod auth;
pub mod discovery;
pub mod envelope;
pub mod events;
/// REST client and protocol types generated from the pinned OpenAPI document.
pub mod generated;
pub mod interaction;
pub mod reconcile;
pub mod session;
pub mod transport;
