//! Tool-call pairing repair applied to a request on its way to a provider.
//!
//! Providers reject a tool result whose originating call is missing from the
//! same request, and the rejection is deterministic: the request never reaches
//! the model, so retrying the same history fails identically every time and the
//! run cannot make progress again.
//!
//! Histories are trimmed in several places — channel compaction, chronicle
//! cuts, fork precompaction, and a worker's own mid-run compaction — and every
//! one of those cuts can land between a call and its result. Each cut aligns
//! itself past a stranded result, which keeps the surrounding turn intact and
//! is the better place to solve it. This pass is the guarantee underneath them:
//! whatever assembled the history, what leaves for the provider pairs.

use rig::message::{AssistantContent, Message, UserContent};
use rig::one_or_many::OneOrMany;
use std::collections::HashSet;

/// Every identifier a provider might pair a result against.
///
/// The converters send `call_id` when it is present and non-empty and fall back
/// to `id`, and the two halves of a pair do not always carry the same field —
/// a result can hold a `call_id` where its call holds only an `id`. Collecting
/// both from the call side and accepting either from the result side keeps the
/// match as permissive as the wire format allows, so a repair only ever removes
/// a result that no call in the request can claim under any pairing rule.
fn collect_call_identifiers(history: &OneOrMany<Message>) -> HashSet<String> {
    let mut identifiers = HashSet::new();

    for message in history.iter() {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        for item in content.iter() {
            let AssistantContent::ToolCall(call) = item else {
                continue;
            };
            if !call.id.is_empty() {
                identifiers.insert(call.id.clone());
            }
            if let Some(call_id) = call.call_id.as_deref().filter(|id| !id.is_empty()) {
                identifiers.insert(call_id.to_string());
            }
        }
    }

    identifiers
}

/// Whether some tool call in the request claims this result.
fn is_claimed(result: &rig::message::ToolResult, identifiers: &HashSet<String>) -> bool {
    identifiers.contains(&result.id)
        || result
            .call_id
            .as_deref()
            .is_some_and(|call_id| identifiers.contains(call_id))
}

/// Drop tool results that no call in `history` claims.
///
/// Returns the repaired history and the number of results dropped, or `None`
/// when every result is already paired — the common case, which allocates
/// nothing beyond the identifier set.
///
/// A user message reduced to nothing is dropped along with its results. Text
/// and images in the same message survive, so a turn that mixes a prompt with a
/// stranded result keeps the prompt.
pub fn repair_orphaned_tool_results(history: &OneOrMany<Message>) -> Option<(Vec<Message>, usize)> {
    let identifiers = collect_call_identifiers(history);

    let orphaned = history
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(content),
            _ => None,
        })
        .flat_map(|content| content.iter())
        .filter(|item| match item {
            UserContent::ToolResult(result) => !is_claimed(result, &identifiers),
            _ => false,
        })
        .count();

    if orphaned == 0 {
        return None;
    }

    let mut repaired = Vec::with_capacity(history.len());
    for message in history.iter() {
        let Message::User { content } = message else {
            repaired.push(message.clone());
            continue;
        };

        let kept: Vec<UserContent> = content
            .iter()
            .filter(|item| match item {
                UserContent::ToolResult(result) => is_claimed(result, &identifiers),
                _ => true,
            })
            .cloned()
            .collect();

        if let Ok(content) = OneOrMany::many(kept) {
            repaired.push(Message::User { content });
        }
    }

    Some((repaired, orphaned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::message::{ToolResult, ToolResultContent};

    fn tool_call(id: &str, call_id: Option<&str>) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::ToolCall(rig::message::ToolCall {
                id: id.to_string(),
                call_id: call_id.map(str::to_string),
                function: rig::message::ToolFunction {
                    name: "shell".to_string(),
                    arguments: serde_json::json!({"command": "ls"}),
                },
                signature: None,
                additional_params: None,
            })),
        }
    }

    fn tool_result(id: &str, call_id: Option<&str>) -> UserContent {
        UserContent::ToolResult(ToolResult {
            id: id.to_string(),
            call_id: call_id.map(str::to_string),
            content: OneOrMany::one(ToolResultContent::text("ok")),
        })
    }

    fn results(items: Vec<UserContent>) -> Message {
        Message::User {
            content: OneOrMany::many(items).expect("non-empty"),
        }
    }

    #[test]
    fn paired_history_is_left_alone() {
        let history = OneOrMany::many(vec![
            tool_call("fc_1", Some("call_1")),
            results(vec![tool_result("call_1", None)]),
        ])
        .expect("non-empty");

        assert!(repair_orphaned_tool_results(&history).is_none());
    }

    /// A cut that lands between a call and its result leaves the result at the
    /// front of the history with nothing to pair against.
    #[test]
    fn stranded_result_is_dropped_with_its_message() {
        let history = OneOrMany::many(vec![
            results(vec![tool_result("call_gone", None)]),
            Message::from("carry on"),
        ])
        .expect("non-empty");

        let (repaired, dropped) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(dropped, 1);
        assert_eq!(repaired.len(), 1);
        assert!(matches!(repaired[0], Message::User { .. }));
    }

    /// Providers pair on `call_id` when the call carries one, so a result
    /// holding that value is claimed even though no `id` matches.
    #[test]
    fn result_matching_only_the_call_id_survives() {
        let history = OneOrMany::many(vec![
            tool_call("fc_abc", Some("call_abc")),
            results(vec![tool_result("call_abc", None)]),
        ])
        .expect("non-empty");

        assert!(repair_orphaned_tool_results(&history).is_none());
    }

    /// The reverse pairing: the call carries only an `id` and the result
    /// carries it as `call_id`.
    #[test]
    fn result_carrying_the_call_id_field_survives() {
        let history = OneOrMany::many(vec![
            tool_call("fc_only", None),
            results(vec![tool_result("unrelated", Some("fc_only"))]),
        ])
        .expect("non-empty");

        assert!(repair_orphaned_tool_results(&history).is_none());
    }

    /// One parallel call batch, one result of which lost its call: the batch's
    /// surviving results stay, and only the stranded one goes.
    #[test]
    fn only_the_unclaimed_result_of_a_batch_is_dropped() {
        let history = OneOrMany::many(vec![
            tool_call("fc_kept", Some("call_kept")),
            results(vec![
                tool_result("call_kept", None),
                tool_result("call_gone", None),
            ]),
        ])
        .expect("non-empty");

        let (repaired, dropped) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(dropped, 1);
        assert_eq!(repaired.len(), 2);
        let Message::User { content } = &repaired[1] else {
            panic!("expected the result message to survive");
        };
        assert_eq!(content.iter().count(), 1);
    }

    /// A history that is nothing but orphans repairs to no messages at all.
    /// `OneOrMany` cannot represent that, so the caller has to turn it into an
    /// error rather than send a request a provider will reject.
    #[test]
    fn an_orphan_only_history_repairs_to_nothing() {
        let history = OneOrMany::many(vec![
            results(vec![tool_result("call_gone", None)]),
            results(vec![tool_result("call_also_gone", None)]),
        ])
        .expect("non-empty");

        let (repaired, dropped) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(dropped, 2);
        assert!(repaired.is_empty());
        assert!(OneOrMany::many(repaired).is_err());
    }

    /// A turn that mixes a stranded result with real prompt text keeps the text.
    #[test]
    fn text_in_a_repaired_message_survives() {
        let history = OneOrMany::many(vec![results(vec![
            tool_result("call_gone", None),
            UserContent::text("what did you find?"),
        ])])
        .expect("non-empty");

        let (repaired, dropped) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(dropped, 1);
        assert_eq!(repaired.len(), 1);
        let Message::User { content } = &repaired[0] else {
            panic!("expected a user message");
        };
        assert!(matches!(
            content.iter().next(),
            Some(UserContent::Text(text)) if text.text == "what did you find?"
        ));
    }
}
