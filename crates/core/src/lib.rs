//! Pure Gentle Shell logic shared by the app: protocol models, parsers and
//! state transitions. No GPUI, no RPC process, no filesystem, no processes,
//! no OS APIs (see docs/adr/0002-nucleo-puro-incremental.md).

pub mod activity;
pub mod commands;
pub mod gentle_state;
pub mod notifications;
pub mod profiles;
pub mod text;
pub mod tools;
pub mod transcript;
pub mod usage;
