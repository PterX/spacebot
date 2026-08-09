//! Authority resolution for slash commands.
//!
//! Access layers *authority* (who may change state) on top of admission (who
//! may talk to the agent at all); it never widens admission. The authority
//! list for a scope resolves binding-first, then the adapter-instance
//! default, and is open when neither is configured — existing configs behave
//! exactly as before.

use super::registry::{CommandAccess, CommandDef, CommandHandler, Surface};
use std::collections::HashMap;

/// Adapter-level default authority lists, keyed by runtime adapter key
/// (`discord`, `telegram:support`, ...). Rebuilt whenever config reloads,
/// alongside the binding swap.
#[derive(Debug, Default, Clone)]
pub struct AdapterAuthorityDefaults {
    by_adapter: HashMap<String, Vec<String>>,
}

impl AdapterAuthorityDefaults {
    pub fn from_config(config: &crate::config::Config) -> Self {
        let mut by_adapter = HashMap::new();
        let mut insert = |key: String, authority: &Vec<String>| {
            if !authority.is_empty() {
                by_adapter.insert(key, authority.clone());
            }
        };

        let messaging = &config.messaging;
        if let Some(discord) = &messaging.discord {
            insert("discord".into(), &discord.authority);
            for instance in &discord.instances {
                insert(format!("discord:{}", instance.name), &instance.authority);
            }
        }
        if let Some(slack) = &messaging.slack {
            insert("slack".into(), &slack.authority);
            for instance in &slack.instances {
                insert(format!("slack:{}", instance.name), &instance.authority);
            }
        }
        if let Some(telegram) = &messaging.telegram {
            insert("telegram".into(), &telegram.authority);
            for instance in &telegram.instances {
                insert(format!("telegram:{}", instance.name), &instance.authority);
            }
        }
        if let Some(twitch) = &messaging.twitch {
            insert("twitch".into(), &twitch.authority);
            for instance in &twitch.instances {
                insert(format!("twitch:{}", instance.name), &instance.authority);
            }
        }
        if let Some(signal) = &messaging.signal {
            insert("signal".into(), &signal.authority);
            for instance in &signal.instances {
                insert(format!("signal:{}", instance.name), &instance.authority);
            }
        }
        if let Some(mattermost) = &messaging.mattermost {
            insert("mattermost".into(), &mattermost.authority);
            for instance in &mattermost.instances {
                insert(format!("mattermost:{}", instance.name), &instance.authority);
            }
        }
        if let Some(email) = &messaging.email {
            insert("email".into(), &email.authority);
            for instance in &email.instances {
                insert(format!("email:{}", instance.name), &instance.authority);
            }
        }

        Self { by_adapter }
    }

    pub fn for_adapter(&self, adapter_key: &str) -> Option<&[String]> {
        self.by_adapter
            .get(adapter_key)
            .map(|authority| authority.as_slice())
    }
}

/// The identity and scope facts needed to answer "may this sender run this
/// command here".
pub struct AccessContext<'a> {
    /// Authority list from the matched binding, if one matched and set it.
    pub binding_authority: Option<&'a [String]>,
    /// Adapter-instance default authority for this message's adapter.
    pub adapter_default: Option<&'a [String]>,
    /// Platform user id (`InboundMessage.sender_id`).
    pub sender_id: &'a str,
    /// Twitch login, compared case-insensitively — Twitch surfaces logins to
    /// users while `sender_id` carries the numeric id.
    pub sender_login: Option<&'a str>,
}

impl AccessContext<'_> {
    /// Whether the sender holds authority in this scope. Open (true for
    /// everyone) when no authority list is configured at either level.
    pub fn is_authority(&self) -> bool {
        let non_empty = |list: &&[String]| !list.is_empty();
        let Some(list) = self
            .binding_authority
            .filter(non_empty)
            .or(self.adapter_default.filter(non_empty))
        else {
            return true;
        };

        list.iter().any(|entry| {
            entry == "*"
                || entry == self.sender_id
                || self
                    .sender_login
                    .is_some_and(|login| entry.eq_ignore_ascii_case(login))
        })
    }

    pub fn allows(&self, def: &CommandDef) -> bool {
        match def.access {
            CommandAccess::Everyone => true,
            CommandAccess::Authority => self.is_authority(),
        }
    }
}

/// Denial reply for an Authority command the sender may not run: names the
/// commands available to them instead of a bare "no".
pub fn denial_text(def: &CommandDef, surface: Surface) -> String {
    let available: Vec<String> = super::REGISTRY
        .defs()
        .iter()
        .filter(|candidate| {
            candidate.access == CommandAccess::Everyone && candidate.availability.on(surface)
        })
        .map(|candidate| format!("/{}", candidate.name))
        .collect();
    format!(
        "/{} requires authority in this channel. available to you: {}",
        def.name,
        available.join(", ")
    )
}

/// Reply for an Agent command with `BusyPolicy::Reject` arriving mid-turn.
pub fn busy_reject_text(def: &CommandDef) -> String {
    format!(
        "/{} can't run while a turn is in flight — /stop cancels the current work",
        def.name
    )
}

/// Acknowledgment for an Agent command queued behind an in-flight turn. A
/// recognized command is never silently swallowed.
pub fn busy_queued_text(def: &CommandDef) -> String {
    debug_assert!(matches!(def.handler, CommandHandler::Agent(_)));
    format!(
        "/{} queued — it runs after the current turn finishes",
        def.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::REGISTRY;

    fn def(name: &str) -> &'static CommandDef {
        REGISTRY.resolve(name).expect("command def")
    }

    fn owned(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| entry.to_string()).collect()
    }

    #[test]
    fn absent_authority_lists_leave_commands_open() {
        let context = AccessContext {
            binding_authority: None,
            adapter_default: None,
            sender_id: "12345",
            sender_login: None,
        };
        assert!(context.is_authority());
        assert!(context.allows(def("quiet")));
    }

    #[test]
    fn empty_lists_are_treated_as_absent() {
        let empty: Vec<String> = Vec::new();
        let context = AccessContext {
            binding_authority: Some(&empty),
            adapter_default: Some(&empty),
            sender_id: "12345",
            sender_login: None,
        };
        assert!(context.is_authority());
    }

    #[test]
    fn binding_authority_gates_authority_commands_only() {
        let authority = owned(&["999"]);
        let context = AccessContext {
            binding_authority: Some(&authority),
            adapter_default: None,
            sender_id: "12345",
            sender_login: None,
        };
        assert!(!context.is_authority());
        assert!(!context.allows(def("quiet")));
        assert!(context.allows(def("status")));
        assert!(context.allows(def("help")));
        assert!(context.allows(def("tasks")));
    }

    #[test]
    fn binding_list_takes_precedence_over_adapter_default() {
        let binding = owned(&["12345"]);
        let adapter = owned(&["999"]);
        let context = AccessContext {
            binding_authority: Some(&binding),
            adapter_default: Some(&adapter),
            sender_id: "12345",
            sender_login: None,
        };
        assert!(context.is_authority());

        let excluded = AccessContext {
            binding_authority: Some(&adapter),
            adapter_default: Some(&binding),
            sender_id: "12345",
            sender_login: None,
        };
        assert!(!excluded.is_authority());
    }

    #[test]
    fn adapter_default_applies_when_binding_has_no_list() {
        let adapter = owned(&["12345"]);
        let context = AccessContext {
            binding_authority: None,
            adapter_default: Some(&adapter),
            sender_id: "12345",
            sender_login: None,
        };
        assert!(context.is_authority());
    }

    #[test]
    fn wildcard_grants_everyone_authority() {
        let authority = owned(&["*"]);
        let context = AccessContext {
            binding_authority: Some(&authority),
            adapter_default: None,
            sender_id: "anyone",
            sender_login: None,
        };
        assert!(context.is_authority());
    }

    #[test]
    fn twitch_logins_match_case_insensitively() {
        let authority = owned(&["StreamerName"]);
        let context = AccessContext {
            binding_authority: Some(&authority),
            adapter_default: None,
            sender_id: "44556677",
            sender_login: Some("streamername"),
        };
        assert!(context.is_authority());
    }

    #[test]
    fn denial_names_available_commands() {
        let text = denial_text(def("quiet"), Surface::Discord);
        assert!(text.starts_with("/quiet requires authority"));
        assert!(text.contains("/status"));
        assert!(text.contains("/help"));
        assert!(!text.contains("/active"));
    }
}
