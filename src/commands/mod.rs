//! Slash commands: the typed registry and its dispatch types.
//!
//! Design: `docs/design-docs/slash-commands.md`.

mod registry;

pub use registry::{
    AgentAction, ArgSpec, COMMANDS, CommandCategory, CommandDef, CommandHandler, CommandRegistry,
    ControlAction, ParseResult, ParsedCommand, REGISTRY,
};
