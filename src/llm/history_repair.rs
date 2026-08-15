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
//!
//! An unpairable result is rewritten as delimited plain text rather than
//! discarded. The content is often the most expensive thing in the history —
//! the output of a long shell command or a file read — and it stays useful to
//! the model as prose once it can no longer be a protocol message. The
//! delimiters mark it as historical data so it cannot read as an instruction.

use rig::message::{AssistantContent, Message, ToolResult, ToolResultContent, UserContent};
use rig::one_or_many::OneOrMany;
use std::collections::{HashMap, HashSet};

/// How much of a historical tool result survives as plain text.
const MAX_UNTRUSTED_RESULT_CHARS: usize = 1_024;

/// What a repair pass changed, for logging and metrics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ToolHistoryRepair {
    /// Results whose call is absent from the request entirely.
    pub orphan_results: usize,
    /// Results that appear at or before the call they claim.
    pub stale_results: usize,
    /// Second and later results claiming a call already answered.
    pub duplicate_results: usize,
    /// Calls left with no result, removed only when a provider rejected them.
    pub unanswered_calls: usize,
}

impl ToolHistoryRepair {
    pub fn changed(&self) -> bool {
        self.total() > 0
    }

    pub fn total(&self) -> usize {
        self.orphan_results + self.stale_results + self.duplicate_results + self.unanswered_calls
    }
}

/// Every identifier a provider might pair against, mapped to the position of
/// the call that carries it.
///
/// The converters send `call_id` when it is present and non-empty and fall back
/// to `id`, and the two halves of a pair do not always carry the same field —
/// a result can hold a `call_id` where its call holds only an `id`. Collecting
/// both from the call side and accepting either from the result side keeps the
/// match as permissive as the wire format allows, so a repair only ever rewrites
/// a result that no call in the request can claim under any pairing rule.
fn call_positions(history: &[Message]) -> HashMap<String, usize> {
    let mut positions = HashMap::new();

    for (index, message) in history.iter().enumerate() {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        for item in content.iter() {
            let AssistantContent::ToolCall(call) = item else {
                continue;
            };
            if !call.id.is_empty() {
                positions.entry(call.id.clone()).or_insert(index);
            }
            if let Some(call_id) = call.call_id.as_deref().filter(|id| !id.is_empty()) {
                positions.entry(call_id.to_string()).or_insert(index);
            }
        }
    }

    positions
}

/// The identifier a pair is keyed on, preferring the one providers send.
fn call_key(call: &rig::message::ToolCall) -> &str {
    call.call_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(&call.id)
}

fn result_key(result: &ToolResult) -> &str {
    result
        .call_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(&result.id)
}

/// Where the call this result claims sits, under either identifier.
fn claiming_call(result: &ToolResult, positions: &HashMap<String, usize>) -> Option<usize> {
    positions
        .get(&result.id)
        .or_else(|| {
            result
                .call_id
                .as_deref()
                .and_then(|call_id| positions.get(call_id))
        })
        .copied()
}

/// Why a result cannot stay a protocol message.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unpairable {
    Orphan,
    Stale,
    Duplicate,
}

impl Unpairable {
    fn reason(self) -> &'static str {
        match self {
            Self::Orphan => "no matching tool call in this request",
            Self::Stale => "result recorded before the call it answers",
            Self::Duplicate => "call already answered by an earlier result",
        }
    }
}

fn classify(
    result: &ToolResult,
    index: usize,
    positions: &HashMap<String, usize>,
    answered: &mut HashSet<String>,
) -> Option<Unpairable> {
    let Some(call_index) = claiming_call(result, positions) else {
        return Some(Unpairable::Orphan);
    };
    if index <= call_index {
        return Some(Unpairable::Stale);
    }
    if !answered.insert(result_key(result).to_string()) {
        return Some(Unpairable::Duplicate);
    }
    None
}

fn bounded_result_text(result: &ToolResult) -> String {
    let mut text = String::new();
    for item in result.content.iter() {
        if !text.is_empty() {
            text.push('\n');
        }
        match item {
            ToolResultContent::Text(value) => text.push_str(&value.text),
            ToolResultContent::Image(_) => text.push_str("[historical image result omitted]"),
        }
        // A string shorter in bytes than the limit cannot exceed it in
        // characters, so the count only runs once there is enough text to
        // matter — a tool result can carry many items and be very long.
        if text.len() >= MAX_UNTRUSTED_RESULT_CHARS
            && text.chars().count() >= MAX_UNTRUSTED_RESULT_CHARS
        {
            break;
        }
    }

    match text.char_indices().nth(MAX_UNTRUSTED_RESULT_CHARS) {
        Some((cut, _)) => {
            let mut bounded = text[..cut].to_string();
            bounded.push_str("…[truncated]");
            bounded
        }
        None => text,
    }
}

/// Rewrite a result as delimited historical data.
///
/// The delimiters are what make this safe to keep: the content came from a
/// tool, so it is not trusted input, and without a frame around it a shell
/// transcript can read as instructions once it is plain text.
fn historical_note(result: &ToolResult, verdict: Unpairable) -> UserContent {
    UserContent::text(format!(
        "[BEGIN UNTRUSTED HISTORICAL TOOL OUTPUT — {}; call id: {}]\n{}\n[END UNTRUSTED HISTORICAL TOOL OUTPUT]",
        verdict.reason(),
        result_key(result),
        bounded_result_text(result)
    ))
}

/// Rewrite tool results this request cannot pair as delimited plain text.
///
/// Returns the repaired history and what changed, or `None` when every result
/// already pairs — the common case, which allocates nothing beyond the
/// identifier map.
pub fn repair_orphaned_tool_results(
    history: &OneOrMany<Message>,
) -> Option<(Vec<Message>, ToolHistoryRepair)> {
    let positions = call_positions(history.iter().cloned().collect::<Vec<_>>().as_slice());

    let mut report = ToolHistoryRepair::default();
    let mut answered = HashSet::new();
    let mut verdicts: Vec<Vec<Option<Unpairable>>> = Vec::with_capacity(history.len());

    for (index, message) in history.iter().enumerate() {
        let Message::User { content } = message else {
            verdicts.push(Vec::new());
            continue;
        };
        let mut row = Vec::new();
        for item in content.iter() {
            let verdict = match item {
                UserContent::ToolResult(result) => {
                    classify(result, index, &positions, &mut answered)
                }
                _ => None,
            };
            match verdict {
                Some(Unpairable::Orphan) => report.orphan_results += 1,
                Some(Unpairable::Stale) => report.stale_results += 1,
                Some(Unpairable::Duplicate) => report.duplicate_results += 1,
                None => {}
            }
            row.push(verdict);
        }
        verdicts.push(row);
    }

    if !report.changed() {
        return None;
    }

    let mut repaired = Vec::with_capacity(history.len());
    for (message, row) in history.iter().zip(verdicts) {
        let Message::User { content } = message else {
            repaired.push(message.clone());
            continue;
        };

        let rewritten: Vec<UserContent> = content
            .iter()
            .zip(row)
            .map(|(item, verdict)| match (item, verdict) {
                (UserContent::ToolResult(result), Some(verdict)) => {
                    historical_note(result, verdict)
                }
                (item, _) => item.clone(),
            })
            .collect();

        if let Ok(content) = OneOrMany::many(rewritten) {
            repaired.push(Message::User { content });
        }
    }

    Some((repaired, report))
}

/// Remove assistant tool calls that nothing in the history answers.
///
/// Anthropic rejects a `tool_use` with no following `tool_result`, which the
/// result-side pass cannot fix because there is no result to rewrite. A call
/// still awaiting its result is the normal shape mid-loop, so this only runs
/// after a provider has already rejected the request, and never touches the
/// final assistant message.
pub fn drop_unanswered_tool_calls(history: &mut Vec<Message>) -> ToolHistoryRepair {
    let mut answered: HashSet<String> = HashSet::new();
    for message in history.iter() {
        let Message::User { content } = message else {
            continue;
        };
        for item in content.iter() {
            if let UserContent::ToolResult(result) = item {
                answered.insert(result.id.clone());
                if let Some(call_id) = result.call_id.clone() {
                    answered.insert(call_id);
                }
            }
        }
    }

    let last_assistant = history
        .iter()
        .rposition(|message| matches!(message, Message::Assistant { .. }));

    let mut report = ToolHistoryRepair::default();
    let mut rebuilt = Vec::with_capacity(history.len());

    for (index, message) in history.drain(..).enumerate() {
        let Message::Assistant { id, content } = message else {
            rebuilt.push(message);
            continue;
        };

        if Some(index) == last_assistant {
            rebuilt.push(Message::Assistant { id, content });
            continue;
        }

        let kept: Vec<AssistantContent> = content
            .into_iter()
            .filter(|item| match item {
                AssistantContent::ToolCall(call) => {
                    let paired = answered.contains(call_key(call))
                        || answered.contains(&call.id)
                        || call
                            .call_id
                            .as_deref()
                            .is_some_and(|id| answered.contains(id));
                    if !paired {
                        report.unanswered_calls += 1;
                    }
                    paired
                }
                _ => true,
            })
            .collect();

        if let Ok(content) = OneOrMany::many(kept) {
            rebuilt.push(Message::Assistant { id, content });
        }
    }

    *history = rebuilt;
    report
}

/// Report the first pairing violation a provider would reject, if any.
///
/// Used for observability after a cut rather than as a gate: a call still
/// awaiting its result is valid mid-loop, so an unanswered trailing call is
/// deliberately not a violation here.
pub fn validate_tool_history(history: &[Message]) -> Result<(), String> {
    let positions = call_positions(history);
    let mut answered = HashSet::new();

    for (index, message) in history.iter().enumerate() {
        let Message::User { content } = message else {
            continue;
        };
        for item in content.iter() {
            let UserContent::ToolResult(result) = item else {
                continue;
            };
            match classify(result, index, &positions, &mut answered) {
                Some(verdict) => {
                    return Err(format!("{}: {}", result_key(result), verdict.reason()));
                }
                None => continue,
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        tool_result_with(id, call_id, "ok")
    }

    fn tool_result_with(id: &str, call_id: Option<&str>, body: &str) -> UserContent {
        UserContent::ToolResult(ToolResult {
            id: id.to_string(),
            call_id: call_id.map(str::to_string),
            content: OneOrMany::one(ToolResultContent::text(body)),
        })
    }

    fn results(items: Vec<UserContent>) -> Message {
        Message::User {
            content: OneOrMany::many(items).expect("non-empty"),
        }
    }

    fn text_of(message: &Message) -> String {
        let Message::User { content } = message else {
            return String::new();
        };
        content
            .iter()
            .filter_map(|item| match item {
                UserContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn paired_history_is_left_alone() {
        let history = OneOrMany::many(vec![
            tool_call("fc_1", Some("call_1")),
            results(vec![tool_result("call_1", None)]),
        ])
        .expect("non-empty");

        assert!(repair_orphaned_tool_results(&history).is_none());
        assert!(validate_tool_history(&history.iter().cloned().collect::<Vec<_>>()).is_ok());
    }

    /// A cut that lands between a call and its result leaves the result at the
    /// front of the history with nothing to pair against. The output it carried
    /// is preserved as prose rather than thrown away.
    #[test]
    fn stranded_result_becomes_untrusted_text() {
        let history = OneOrMany::many(vec![
            results(vec![tool_result_with(
                "call_gone",
                None,
                "total 48\ndrwxr-xr-x",
            )]),
            Message::from("carry on"),
        ])
        .expect("non-empty");

        let (repaired, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.orphan_results, 1);
        assert_eq!(repaired.len(), 2);

        let note = text_of(&repaired[0]);
        assert!(note.contains("UNTRUSTED HISTORICAL TOOL OUTPUT"));
        assert!(note.contains("no matching tool call"));
        assert!(note.contains("call_gone"));
        assert!(note.contains("drwxr-xr-x"), "the output itself survives");

        // Nothing pairs any more, so the request is now valid.
        assert!(validate_tool_history(&repaired).is_ok());
    }

    /// The shape that took down a live worker: a fork's compaction cut removed
    /// the assistant turn holding the first `read_skill` call while its result
    /// stayed at the head of the retained history.
    #[test]
    fn a_forked_worker_history_cut_mid_turn_is_repaired() {
        let history = OneOrMany::many(vec![
            results(vec![tool_result_with(
                "call_HPJ4d0Mb42LJt6JzCqYcwRsq",
                None,
                "# Skill: instance-debugging",
            )]),
            tool_call("fc_next", Some("call_next")),
            results(vec![tool_result("call_next", None)]),
        ])
        .expect("non-empty");

        assert!(
            validate_tool_history(&history.iter().cloned().collect::<Vec<_>>()).is_err(),
            "the history a provider rejected must fail validation"
        );

        let (repaired, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.orphan_results, 1);
        assert_eq!(report.total(), 1, "the intact pair is untouched");
        assert!(validate_tool_history(&repaired).is_ok());
        assert!(text_of(&repaired[0]).contains("instance-debugging"));
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
    /// surviving results stay protocol messages and only the stranded one is
    /// rewritten.
    #[test]
    fn only_the_unclaimed_result_of_a_batch_is_rewritten() {
        let history = OneOrMany::many(vec![
            tool_call("fc_kept", Some("call_kept")),
            results(vec![
                tool_result("call_kept", None),
                tool_result("call_gone", None),
            ]),
        ])
        .expect("non-empty");

        let (repaired, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.orphan_results, 1);

        let Message::User { content } = &repaired[1] else {
            panic!("expected the result message to survive");
        };
        let kinds: Vec<bool> = content
            .iter()
            .map(|item| matches!(item, UserContent::ToolResult(_)))
            .collect();
        assert_eq!(kinds, vec![true, false], "one stays a result, one is prose");
    }

    /// A second result for a call already answered is rejected by providers as
    /// firmly as an orphan.
    #[test]
    fn a_duplicate_result_is_rewritten() {
        let history = OneOrMany::many(vec![
            tool_call("fc_1", Some("call_1")),
            results(vec![tool_result("call_1", None)]),
            results(vec![tool_result("call_1", None)]),
        ])
        .expect("non-empty");

        let (repaired, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.duplicate_results, 1);
        assert_eq!(report.orphan_results, 0);
        assert!(validate_tool_history(&repaired).is_ok());
    }

    /// A result placed at or before its call cannot be paired in order.
    #[test]
    fn a_stale_result_is_rewritten() {
        let history = OneOrMany::many(vec![
            results(vec![tool_result("call_1", None)]),
            tool_call("fc_1", Some("call_1")),
        ])
        .expect("non-empty");

        let (_, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.stale_results, 1);
    }

    /// A turn that mixes a stranded result with real prompt text keeps the text.
    #[test]
    fn text_in_a_repaired_message_survives() {
        let history = OneOrMany::many(vec![results(vec![
            tool_result("call_gone", None),
            UserContent::text("what did you find?"),
        ])])
        .expect("non-empty");

        let (repaired, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.orphan_results, 1);
        assert!(text_of(&repaired[0]).contains("what did you find?"));
    }

    /// Every message is preserved, so a history of nothing but orphans still
    /// produces a sendable request instead of an empty one.
    #[test]
    fn an_orphan_only_history_still_produces_messages() {
        let history = OneOrMany::many(vec![
            results(vec![tool_result("call_gone", None)]),
            results(vec![tool_result("call_also_gone", None)]),
        ])
        .expect("non-empty");

        let (repaired, report) = repair_orphaned_tool_results(&history).expect("repair");
        assert_eq!(report.orphan_results, 2);
        assert_eq!(repaired.len(), 2);
        assert!(OneOrMany::many(repaired).is_ok());
    }

    /// Long output is bounded so a repair cannot blow the context it was
    /// trimmed to fit.
    #[test]
    fn a_long_result_is_truncated_in_the_note() {
        let body = "x".repeat(MAX_UNTRUSTED_RESULT_CHARS * 3);
        let history = OneOrMany::many(vec![results(vec![tool_result_with("gone", None, &body)])])
            .expect("non-empty");

        let (repaired, _) = repair_orphaned_tool_results(&history).expect("repair");
        let note = text_of(&repaired[0]);
        assert!(note.contains("…[truncated]"));
        assert!(note.chars().count() < MAX_UNTRUSTED_RESULT_CHARS + 300);
    }

    /// An unanswered call mid-history is what Anthropic rejects; the trailing
    /// one is a loop still in flight and must survive.
    #[test]
    fn only_a_non_trailing_unanswered_call_is_dropped() {
        let mut history = vec![
            tool_call("fc_dead", Some("call_dead")),
            Message::from("unrelated turn"),
            tool_call("fc_live", Some("call_live")),
        ];

        let report = drop_unanswered_tool_calls(&mut history);

        assert_eq!(report.unanswered_calls, 1);
        assert_eq!(history.len(), 2, "the emptied assistant message goes too");
        assert!(matches!(history[1], Message::Assistant { .. }));
    }

    #[test]
    fn answered_calls_are_never_dropped() {
        let mut history = vec![
            tool_call("fc_1", Some("call_1")),
            results(vec![tool_result("call_1", None)]),
            Message::from("later"),
        ];

        let report = drop_unanswered_tool_calls(&mut history);

        assert_eq!(report.unanswered_calls, 0);
        assert_eq!(history.len(), 3);
    }
}
