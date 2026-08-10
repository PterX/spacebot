//! Slash commands: the typed registry and its dispatch types.
//!
//! Design: `docs/design-docs/slash-commands.md`.

pub mod access;
pub mod control;
pub mod dispatch;
pub mod native;
pub mod registry;

pub use registry::{
    AgentAction, ArgSpec, BusyPolicy, COMMANDS, CommandAccess, CommandAvailability,
    CommandCategory, CommandDef, CommandHandler, CommandRegistry, ControlAction, ParseResult,
    ParsedCommand, REGISTRY, Surface,
};
