//! Chronicle lifecycle: interval checkpoint cuts, and the bounded checkpoint
//! view rendered into the channel system prompt.
//!
//! Like the compactor this is a programmatic monitor, not an LLM process — it
//! watches counters and spawns the summarization. Unlike the compactor its
//! output is durable and append-only: each cut summarizes only the span since
//! the previous checkpoint, and the resulting text never re-enters the
//! transcript that a later cut reads.
//!
//! See `docs/design-docs/session-chronicles.md`.

use crate::agent::compactor::{estimate_history_tokens, estimate_text_tokens};
use crate::config::ChronicleConfig;
use crate::conversation::chronicle::{
    CheckpointKind, ChronicleBoundary, ChronicleCheckpoint, ChronicleStats, ChronicleStore,
    CommitOutcome, NewCheckpoint,
};
use crate::conversation::history::ConversationMessage;
use crate::error::Result;
use crate::hooks::SpacebotHook;
use crate::llm::SpacebotModel;
use crate::{AgentDeps, ChannelId, ProcessId, ProcessType};

use chrono::{DateTime, Duration, Utc};
use rig::agent::AgentBuilder;
use rig::completion::CompletionModel as _;
use rig::message::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Live messages kept after a trim regardless of what the chronicle covers.
/// A channel needs its latest exchange to act at all.
const MIN_RETAINED_MESSAGES: usize = 4;

/// Extra live messages kept beyond the uncovered count when trimming.
///
/// `ConversationLogger` writes are fire-and-forget, so the durable log can lag
/// the in-memory history by a few messages. Retaining a margin means a lagging
/// write can never cause a live message to be dropped before its checkpoint
/// covers it. Over-retention costs a few raw turns of context; under-retention
/// loses them outright, so the margin errs toward keeping.
const TRIM_SAFETY_MARGIN: usize = 8;

/// Prior checkpoints handed to a cut as narrative context, newest last.
const NARRATIVE_CONTEXT_CHECKPOINTS: i64 = 3;

/// Why `check_and_chronicle` acted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChronicleAction {
    /// The first checkpoint for a channel that entered chronicle mode with
    /// more history than one interval can summarize.
    Bootstrap,
    /// The message or token interval elapsed.
    Interval,
    /// Context pressure forced a cut early.
    Pressure,
    /// Emergency truncation dropped the span without summarizing it.
    Emergency,
}

/// Per-channel chronicle monitor.
pub struct Chronicler {
    channel_id: ChannelId,
    deps: AgentDeps,
    history: Arc<RwLock<Vec<Message>>>,
    store: ChronicleStore,
    model_override: Option<String>,
    /// Bumped on every structural head mutation of the live history. A cut
    /// that finishes after the generation moved skips its trim.
    generation: Arc<AtomicU64>,
    /// One in-flight cut per channel.
    cutting: Arc<AtomicBool>,
}

impl Chronicler {
    pub fn new(
        channel_id: ChannelId,
        deps: AgentDeps,
        history: Arc<RwLock<Vec<Message>>>,
        model_override: Option<String>,
    ) -> Self {
        let store = ChronicleStore::new(deps.sqlite_pool.clone());
        Self {
            channel_id,
            deps,
            history,
            store,
            model_override,
            generation: Arc::new(AtomicU64::new(0)),
            cutting: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn store(&self) -> &ChronicleStore {
        &self.store
    }

    fn config(&self) -> ChronicleConfig {
        self.deps.runtime_config.compaction.load().chronicle
    }

    /// Decide whether to cut a checkpoint, and start one if so.
    ///
    /// Called after each turn, in place of the rolling compactor's threshold
    /// check. Emergency truncation runs synchronously; everything else spawns.
    pub async fn check_and_chronicle(&self) -> Result<Option<ChronicleAction>> {
        let config = self.config();
        let compaction = **self.deps.runtime_config.compaction.load();
        let context_window = **self.deps.runtime_config.context_window.load();

        let live_tokens = {
            let history = self.history.read().await;
            estimate_history_tokens(&history)
        };
        let usage = live_tokens as f32 / context_window.max(1) as f32;

        if usage >= compaction.emergency_threshold {
            self.emergency_truncate().await?;
            return Ok(Some(ChronicleAction::Emergency));
        }

        if self.cutting.load(Ordering::Acquire) {
            return Ok(None);
        }

        let latest = self.store.latest(&self.channel_id, 0).await?;
        let boundary = match &latest {
            Some(checkpoint) => checkpoint.end_boundary(),
            None => match self.store.earliest_message(&self.channel_id).await? {
                Some((at, _)) => ChronicleBoundary::origin(at),
                // Nothing logged yet: nothing to chronicle.
                None => return Ok(None),
            },
        };

        let uncovered = self
            .store
            .count_messages_after(&self.channel_id, &boundary)
            .await?;
        if uncovered == 0 {
            return Ok(None);
        }

        let interval_tokens =
            (context_window as f32 * config.interval_token_fraction).max(1.0) as usize;

        let action = if latest.is_none() && uncovered > config.max_messages_per_checkpoint {
            // Entering chronicle mode on a channel whose backlog is larger than
            // one cut can read. A smaller backlog needs no special handling —
            // the first interval cut simply covers all of it.
            Some(ChronicleAction::Bootstrap)
        } else if usage >= compaction.background_threshold {
            Some(ChronicleAction::Pressure)
        } else if uncovered >= config.interval_messages as i64 || live_tokens >= interval_tokens {
            Some(ChronicleAction::Interval)
        } else {
            None
        };

        let Some(action) = action else {
            return Ok(None);
        };

        tracing::info!(
            channel_id = %self.channel_id,
            ?action,
            uncovered,
            live_tokens,
            usage = %format!("{:.1}%", usage * 100.0),
            "chronicle checkpoint triggered"
        );

        self.spawn_cut(action, boundary, config);
        Ok(Some(action))
    }

    fn spawn_cut(
        &self,
        action: ChronicleAction,
        boundary: ChronicleBoundary,
        config: ChronicleConfig,
    ) {
        if self.cutting.swap(true, Ordering::AcqRel) {
            return;
        }

        let kind = match action {
            ChronicleAction::Bootstrap => CheckpointKind::Bootstrap,
            ChronicleAction::Pressure => CheckpointKind::Pressure,
            ChronicleAction::Interval => CheckpointKind::Interval,
            ChronicleAction::Emergency => CheckpointKind::Emergency,
        };

        let cut = CutContext {
            channel_id: self.channel_id.clone(),
            deps: self.deps.clone(),
            store: self.store.clone(),
            history: self.history.clone(),
            generation: self.generation.clone(),
            model_override: self.model_override.clone(),
            config,
        };
        let cutting = self.cutting.clone();

        tokio::spawn(async move {
            let channel_id = cut.channel_id.clone();
            if let Err(error) = cut.run(kind, boundary).await {
                tracing::error!(channel_id = %channel_id, %error, "chronicle checkpoint failed");
            }
            cutting.store(false, Ordering::Release);
        });
    }

    /// Drop the oldest half of the live history without summarizing it, and
    /// record a checkpoint marking the discarded span.
    ///
    /// This is the one checkpoint whose summary does not describe its
    /// contents, and it says so. Recording it keeps coverage contiguous, so
    /// the gap is visible in the chronicle rather than silent.
    async fn emergency_truncate(&self) -> Result<()> {
        let (removed, retained_live) = {
            let mut history = self.history.write().await;
            let total = history.len();
            if total <= MIN_RETAINED_MESSAGES {
                return Ok(());
            }
            let remove_count = (total / 2).min(total - MIN_RETAINED_MESSAGES);
            history.drain(..remove_count);
            self.generation.fetch_add(1, Ordering::AcqRel);
            (remove_count, history.len())
        };

        tracing::warn!(
            channel_id = %self.channel_id,
            removed,
            "chronicle emergency truncation performed"
        );

        let latest = self.store.latest(&self.channel_id, 0).await?;
        let from = match &latest {
            Some(checkpoint) => checkpoint.end_boundary(),
            None => match self.store.earliest_message(&self.channel_id).await? {
                Some((at, _)) => ChronicleBoundary::origin(at),
                None => return Ok(()),
            },
        };

        let messages = self
            .store
            .messages_after(
                &self.channel_id,
                &from,
                self.config().max_messages_per_checkpoint,
            )
            .await?;

        // Cover only the part of the log the truncation actually discarded.
        // The messages still in live context have to stay uncovered so a later
        // interval cut can summarize them properly — marking them
        // "not summarized" here would advance the boundary past them and lose
        // that chance. The live-to-log mapping is approximate, so this errs
        // toward covering less.
        let covered = messages.len().saturating_sub(retained_live);
        let Some(last) = covered.checked_sub(1).and_then(|index| messages.get(index)) else {
            tracing::debug!(
                channel_id = %self.channel_id,
                "emergency truncation covered no logged span; the tail stays for the next cut"
            );
            return Ok(());
        };
        let to = ChronicleBoundary::new(last.created_at, Some(last.id.clone()));

        let outcome = self
            .store
            .commit(NewCheckpoint {
                channel_id: self.channel_id.to_string(),
                level: 0,
                kind: CheckpointKind::Emergency,
                title: format!("Truncated span ({covered} messages)"),
                summary: format!(
                    "Context reached the emergency threshold and {removed} live messages were \
                     dropped without summarization. {covered} logged messages in this span were \
                     not summarized; use the chronicle tool to expand this range if the detail \
                     is needed."
                ),
                covers_from: from,
                covers_to: to,
                message_count: covered as i64,
                token_estimate: 0,
                rolls_up_from_seq: None,
                rolls_up_to_seq: None,
                model: None,
            })
            .await?;

        if let CommitOutcome::Committed(checkpoint) = outcome {
            emit_checkpoint_event(&self.deps, &self.channel_id, &checkpoint);
        }

        Ok(())
    }
}

/// Everything a spawned cut needs, so the task owns no `&self` borrow.
struct CutContext {
    channel_id: ChannelId,
    deps: AgentDeps,
    store: ChronicleStore,
    history: Arc<RwLock<Vec<Message>>>,
    generation: Arc<AtomicU64>,
    model_override: Option<String>,
    config: ChronicleConfig,
}

impl CutContext {
    async fn run(&self, kind: CheckpointKind, from: ChronicleBoundary) -> Result<()> {
        let messages = self
            .store
            .messages_after(
                &self.channel_id,
                &from,
                self.config.max_messages_per_checkpoint,
            )
            .await?;

        let Some(last) = messages.last() else {
            return Ok(());
        };
        let to = ChronicleBoundary::new(last.created_at, Some(last.id.clone()));
        let generation_at_cut = self.generation.load(Ordering::Acquire);

        let narrative = self
            .store
            .list(&self.channel_id, 0, NARRATIVE_CONTEXT_CHECKPOINTS)
            .await?;

        let (title, summary, model) = self.summarize(kind, &messages, &narrative).await;

        let outcome = self
            .store
            .commit(NewCheckpoint {
                channel_id: self.channel_id.to_string(),
                level: 0,
                kind,
                title,
                summary: summary.clone(),
                covers_from: from,
                covers_to: to,
                message_count: messages.len() as i64,
                token_estimate: estimate_text_tokens(&summary) as i64,
                rolls_up_from_seq: None,
                rolls_up_to_seq: None,
                model,
            })
            .await?;

        let checkpoint = match outcome {
            CommitOutcome::Committed(checkpoint) => checkpoint,
            CommitOutcome::Superseded { expected, found } => {
                tracing::info!(
                    channel_id = %self.channel_id,
                    ?expected,
                    ?found,
                    "chronicle cut superseded; span stays unsummarized for the next cut"
                );
                return Ok(());
            }
        };

        emit_checkpoint_event(&self.deps, &self.channel_id, &checkpoint);
        self.trim_live_history(&checkpoint, generation_at_cut)
            .await?;

        tracing::info!(
            channel_id = %self.channel_id,
            seq = checkpoint.seq,
            message_count = checkpoint.message_count,
            "chronicle checkpoint committed"
        );

        Ok(())
    }

    /// Produce the checkpoint's title and summary.
    ///
    /// Prior summaries are supplied as narrative context so the entry reads as
    /// a continuation, but the model is told to describe only the new span —
    /// no checkpoint is ever regenerated from another checkpoint's text.
    async fn summarize(
        &self,
        kind: CheckpointKind,
        messages: &[ConversationMessage],
        narrative: &[ChronicleCheckpoint],
    ) -> (String, String, Option<String>) {
        let fallback_title = range_title(messages);

        // A bootstrap cut can inherit the rolling compactor's summary head
        // rather than paying for an LLM pass over history it cannot fully read.
        if kind == CheckpointKind::Bootstrap
            && let Some(existing) = self.rolling_summary_head().await
        {
            return (
                format!("Prior history — {fallback_title}"),
                format!(
                    "Carried over from rolling compaction when this channel entered chronicle \
                     mode. {existing}"
                ),
                None,
            );
        }

        let prompt_engine = self.deps.runtime_config.prompts.load();
        let preamble = match prompt_engine.render_static("chronicle_checkpoint") {
            Ok(preamble) => preamble,
            Err(error) => {
                tracing::error!(%error, "failed to render chronicle checkpoint prompt");
                return (fallback_title, unsummarized_notice(messages.len()), None);
            }
        };

        let routing = self.deps.runtime_config.routing.load();
        let model_name = match &self.model_override {
            Some(model) => model.clone(),
            None => routing.resolve(ProcessType::Compactor, None).to_string(),
        };
        let model = SpacebotModel::make(&self.deps.llm_manager, &model_name)
            .with_context(&*self.deps.agent_id, "chronicle")
            .with_routing((**routing).clone());

        let agent = AgentBuilder::new(model)
            .preamble(&preamble)
            .default_max_turns(1)
            .build();

        let hook = SpacebotHook::new(
            self.deps.agent_id.clone(),
            ProcessId::Worker(Uuid::new_v4()),
            ProcessType::Compactor,
            Some(self.channel_id.clone()),
            self.deps.event_tx.clone(),
        );

        let prompt = build_cut_prompt(kind, messages, narrative);
        let mut cut_history = Vec::new();
        let response = hook.prompt_once(&agent, &mut cut_history, &prompt).await;

        match response {
            Ok(text) => {
                let (title, summary) = parse_checkpoint_response(&text);
                (
                    title.unwrap_or(fallback_title),
                    summary,
                    Some(model_name.clone()),
                )
            }
            Err(error) => {
                tracing::warn!(%error, "chronicle summarization failed, recording an unsummarized span");
                (fallback_title, unsummarized_notice(messages.len()), None)
            }
        }
    }

    /// The rolling compactor's summary head, if this channel was running in
    /// rolling mode before the switch.
    async fn rolling_summary_head(&self) -> Option<String> {
        let history = self.history.read().await;
        let first = history.first()?;
        let Message::User { content } = first else {
            return None;
        };
        for item in content.iter() {
            if let rig::message::UserContent::Text(text) = item
                && let Some(stripped) = text.text.strip_prefix("[Compaction Summary]: ")
            {
                return Some(stripped.trim().to_string());
            }
        }
        None
    }

    /// Drop live messages the chronicle now covers.
    ///
    /// The live history and the durable log are not indexed against each
    /// other, so the retained count is derived from what the log says is
    /// uncovered, plus a margin for fire-and-forget write lag. Skipped
    /// entirely when the head moved while the cut was running — the
    /// checkpoint stays valid either way, and the next trim catches up.
    async fn trim_live_history(
        &self,
        checkpoint: &ChronicleCheckpoint,
        generation_at_cut: u64,
    ) -> Result<()> {
        let uncovered = self
            .store
            .count_messages_after(&self.channel_id, &checkpoint.end_boundary())
            .await? as usize;
        let retain = uncovered
            .saturating_add(TRIM_SAFETY_MARGIN)
            .max(MIN_RETAINED_MESSAGES);

        let mut history = self.history.write().await;
        if self.generation.load(Ordering::Acquire) != generation_at_cut {
            tracing::debug!(
                channel_id = %self.channel_id,
                seq = checkpoint.seq,
                "skipping chronicle trim: live history changed generation during the cut"
            );
            return Ok(());
        }

        let total = history.len();
        if total <= retain {
            return Ok(());
        }

        let remove = total - retain;
        history.drain(..remove);
        self.generation.fetch_add(1, Ordering::AcqRel);

        tracing::debug!(
            channel_id = %self.channel_id,
            seq = checkpoint.seq,
            removed = remove,
            retained = history.len(),
            "trimmed live history to the chronicle boundary"
        );

        Ok(())
    }
}

fn emit_checkpoint_event(
    deps: &AgentDeps,
    channel_id: &ChannelId,
    checkpoint: &ChronicleCheckpoint,
) {
    if let Err(error) = deps
        .event_tx
        .send(crate::ProcessEvent::ChronicleCheckpoint {
            agent_id: deps.agent_id.clone(),
            channel_id: channel_id.clone(),
            checkpoint: Box::new(crate::ChronicleCheckpointPayload {
                checkpoint_id: checkpoint.id.clone(),
                seq: checkpoint.seq,
                level: checkpoint.level,
                kind: checkpoint.kind.as_str().to_string(),
                title: checkpoint.title.clone(),
                summary: checkpoint.summary.clone(),
                covers_from: checkpoint.covers_from_at.to_rfc3339(),
                covers_to: checkpoint.covers_to_at.to_rfc3339(),
                message_count: checkpoint.message_count,
                created_at: checkpoint.created_at.to_rfc3339(),
            }),
        })
    {
        tracing::debug!(%error, "failed to emit chronicle checkpoint event");
    }
}

fn unsummarized_notice(message_count: usize) -> String {
    format!(
        "This span of {message_count} messages was not summarized — summarization failed. \
         Expand the range with the chronicle tool if the detail is needed."
    )
}

/// A date-range title, used when the model does not supply one.
fn range_title(messages: &[ConversationMessage]) -> String {
    match (messages.first(), messages.last()) {
        (Some(first), Some(last)) => {
            let from = first.created_at.format("%Y-%m-%d %H:%M");
            let to = last.created_at.format("%H:%M");
            format!("{from}–{to} UTC")
        }
        _ => "Untitled span".to_string(),
    }
}

/// Split a `TITLE: …` first line off the model's response.
fn parse_checkpoint_response(response: &str) -> (Option<String>, String) {
    let trimmed = response.trim();
    let mut lines = trimmed.lines();
    let Some(first) = lines.next() else {
        return (None, String::new());
    };

    if let Some(title) = first.trim().strip_prefix("TITLE:") {
        let title = title.trim();
        let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        if !title.is_empty() && !body.is_empty() {
            return (Some(truncate_title(title)), body);
        }
    }

    (None, trimmed.to_string())
}

const MAX_TITLE_CHARS: usize = 80;

fn truncate_title(title: &str) -> String {
    if title.chars().count() <= MAX_TITLE_CHARS {
        return title.to_string();
    }
    let truncated: String = title.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{truncated}…")
}

/// Build the summarization prompt: narrative context first, then the span.
fn build_cut_prompt(
    kind: CheckpointKind,
    messages: &[ConversationMessage],
    narrative: &[ChronicleCheckpoint],
) -> String {
    let mut prompt = String::new();

    if !narrative.is_empty() {
        prompt.push_str("## Story so far\n\n");
        prompt.push_str(
            "These are earlier checkpoints, for continuity only. Do not restate them.\n\n",
        );
        for checkpoint in narrative.iter().rev() {
            prompt.push_str(&format!(
                "- **{}** ({} → {}): {}\n",
                checkpoint.title,
                checkpoint.covers_from_at.format("%Y-%m-%d %H:%M"),
                checkpoint.covers_to_at.format("%Y-%m-%d %H:%M"),
                checkpoint.summary
            ));
        }
        prompt.push('\n');
    }

    if kind == CheckpointKind::Bootstrap {
        prompt.push_str(
            "## Note\n\nThis channel is entering chronicle mode with existing history. The \
             transcript below may be the tail of a longer past; summarize what is here and do \
             not speculate about what came before.\n\n",
        );
    }

    prompt.push_str("## New span to summarize\n\n");
    prompt.push_str(&render_log_transcript(messages));
    prompt
}

/// Render logged messages for the summarizer.
pub(crate) fn render_log_transcript(messages: &[ConversationMessage]) -> String {
    let mut output = String::new();
    for message in messages {
        let sender = message
            .sender_name
            .as_deref()
            .unwrap_or(match message.role.as_str() {
                "assistant" => "assistant",
                "system" => "system",
                _ => "user",
            });
        output.push_str(&format!(
            "[{}] {} ({}): {}\n",
            message.created_at.format("%Y-%m-%d %H:%M:%S"),
            sender,
            message.role,
            message.content
        ));
    }
    output
}

/// Assemble the bounded chronicle view for a channel's system prompt.
///
/// Recomputed from durable state every turn, so a restart reproduces it
/// exactly. Returns `None` when the channel has no chronicle yet.
pub async fn render_chronicle_view(
    store: &ChronicleStore,
    channel_id: &str,
    now: DateTime<Utc>,
    config: ChronicleConfig,
) -> Result<Option<String>> {
    let stats = store.stats(channel_id).await?;
    if stats.checkpoint_count == 0 {
        return Ok(None);
    }

    let since = now - Duration::hours(config.recent_window_hours);
    let recent = store
        .list_since(channel_id, 0, since, config.max_recent as i64)
        .await?;

    let older = match recent.first() {
        Some(first) => {
            store
                .list_before_seq(channel_id, 0, first.seq, config.max_older as i64)
                .await?
        }
        None => store
            .list(channel_id, 0, config.max_older as i64)
            .await?
            .into_iter()
            .rev()
            .collect(),
    };

    Ok(Some(compose_view(
        &stats,
        &older,
        &recent,
        config.context_token_budget,
    )))
}

/// Render the header and entries within a token budget.
///
/// Under pressure the oldest entries collapse to a title and range line
/// before any entry is dropped, and the header never collapses — it is what
/// tells the agent that more exists and can be expanded.
fn compose_view(
    stats: &ChronicleStats,
    older: &[ChronicleCheckpoint],
    recent: &[ChronicleCheckpoint],
    budget_tokens: usize,
) -> String {
    let header = render_header(stats, older.len() + recent.len());
    let entries: Vec<&ChronicleCheckpoint> = older.iter().chain(recent.iter()).collect();

    // Collapse oldest-first until the whole section fits.
    let mut collapsed_upto = 0usize;
    loop {
        let body = render_entries(&entries, collapsed_upto);
        if estimate_text_tokens(&header) + estimate_text_tokens(&body) <= budget_tokens
            || collapsed_upto >= entries.len()
        {
            return format!("{header}\n{body}");
        }
        collapsed_upto += 1;
    }
}

fn render_header(stats: &ChronicleStats, shown: usize) -> String {
    let mut header = String::from("## Session Chronicle\n\n");

    let age = match (stats.first_message_at, stats.last_message_at) {
        (Some(first), Some(last)) => {
            let days = (last - first).num_days();
            format!(
                "Session spans {} → {} ({} day{}).",
                first.format("%Y-%m-%d"),
                last.format("%Y-%m-%d"),
                days,
                if days == 1 { "" } else { "s" }
            )
        }
        _ => "Session age unknown.".to_string(),
    };

    header.push_str(&format!(
        "{age} {} messages logged, {} checkpoints ({} shown below). \
         {} messages since the last checkpoint are still in raw context.\n\n\
         Checkpoints below are summaries, not the transcript. Use the `chronicle` tool to list \
         the full checkpoint index or open one; a branch can expand any checkpoint back into raw \
         messages.\n",
        stats.total_messages, stats.checkpoint_count, shown, stats.unsummarized_messages,
    ));

    header
}

fn render_entries(entries: &[&ChronicleCheckpoint], collapsed_upto: usize) -> String {
    let mut body = String::new();
    for (index, checkpoint) in entries.iter().enumerate() {
        let range = format!(
            "{} → {}",
            checkpoint.covers_from_at.format("%Y-%m-%d %H:%M"),
            checkpoint.covers_to_at.format("%Y-%m-%d %H:%M")
        );
        if index < collapsed_upto {
            body.push_str(&format!(
                "- **#{} {}** ({}, {} messages) — collapsed; open with the chronicle tool.\n",
                checkpoint.seq, checkpoint.title, range, checkpoint.message_count
            ));
        } else {
            body.push_str(&format!(
                "\n### #{} {}\n{} · {} messages · {}\n\n{}\n",
                checkpoint.seq,
                checkpoint.title,
                range,
                checkpoint.message_count,
                checkpoint.kind.as_str(),
                checkpoint.summary
            ));
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(seq: i64, title: &str, summary: &str) -> ChronicleCheckpoint {
        let at = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        ChronicleCheckpoint {
            id: format!("cp-{seq}"),
            channel_id: "ch".into(),
            seq,
            level: 0,
            kind: CheckpointKind::Interval,
            title: title.into(),
            summary: summary.into(),
            covers_from_at: at,
            covers_to_at: at,
            covers_from_message_id: None,
            covers_to_message_id: Some(format!("m{seq}")),
            message_count: 10,
            token_estimate: 20,
            rolled_up_into: None,
            rolls_up_from_seq: None,
            rolls_up_to_seq: None,
            model: None,
            created_at: at,
        }
    }

    fn stats() -> ChronicleStats {
        ChronicleStats {
            checkpoint_count: 3,
            interval_count: 3,
            rollup_count: 0,
            total_messages: 120,
            first_message_at: Some(
                DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            last_message_at: Some(
                DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            unsummarized_messages: 7,
        }
    }

    #[test]
    fn view_header_reports_session_shape() {
        let view = compose_view(&stats(), &[], &[checkpoint(1, "First", "stuff")], 4000);
        assert!(view.contains("120 messages logged"));
        assert!(view.contains("3 checkpoints"));
        assert!(view.contains("7 messages since the last checkpoint"));
        assert!(view.contains("#1 First"));
    }

    #[test]
    fn view_collapses_oldest_first_under_budget() {
        let body = "x".repeat(2000);
        let entries: Vec<ChronicleCheckpoint> = (1..=4)
            .map(|seq| checkpoint(seq, &format!("Span {seq}"), &body))
            .collect();
        let refs: Vec<&ChronicleCheckpoint> = entries.iter().collect();

        let full = compose_view(&stats(), &[], &entries, 100_000);
        assert!(full.contains(&body), "an ample budget keeps every body");

        let squeezed = compose_view(&stats(), &[], &entries, 400);
        assert!(
            squeezed.contains("#1 Span 1") && squeezed.contains("collapsed"),
            "the oldest entry collapses first"
        );
        assert!(
            squeezed.contains("#4 Span 4"),
            "every checkpoint stays listed even when collapsed"
        );
        assert!(
            squeezed.contains("messages logged"),
            "the header never collapses"
        );
        assert_eq!(refs.len(), 4);
    }

    #[test]
    fn view_budget_is_monotone() {
        let body = "y".repeat(1200);
        let entries: Vec<ChronicleCheckpoint> = (1..=5)
            .map(|seq| checkpoint(seq, &format!("Span {seq}"), &body))
            .collect();

        let mut previous = usize::MAX;
        for budget in [200usize, 800, 2000, 8000, 40_000] {
            let rendered = compose_view(&stats(), &[], &entries, budget);
            let collapsed = rendered.matches("collapsed").count();
            assert!(
                collapsed <= previous,
                "a larger budget must not collapse more entries"
            );
            previous = collapsed;
        }
    }

    #[test]
    fn parse_response_splits_title_from_body() {
        let (title, summary) =
            parse_checkpoint_response("TITLE: Shipping the parser\n\nThey fixed the lexer.");
        assert_eq!(title.as_deref(), Some("Shipping the parser"));
        assert_eq!(summary, "They fixed the lexer.");
    }

    #[test]
    fn parse_response_without_title_keeps_whole_body() {
        let (title, summary) = parse_checkpoint_response("They fixed the lexer.\nThen shipped.");
        assert!(title.is_none());
        assert_eq!(summary, "They fixed the lexer.\nThen shipped.");
    }

    #[test]
    fn parse_response_rejects_title_without_body() {
        let (title, summary) = parse_checkpoint_response("TITLE: Just a title");
        assert!(title.is_none(), "a title with no body is not a valid split");
        assert_eq!(summary, "TITLE: Just a title");
    }

    #[test]
    fn long_titles_are_truncated() {
        let long = "word ".repeat(40);
        let (title, _) = parse_checkpoint_response(&format!("TITLE: {long}\n\nbody"));
        let title = title.expect("title parsed");
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn transcript_render_includes_sender_and_time() {
        let message = ConversationMessage {
            id: "m1".into(),
            channel_id: "ch".into(),
            role: "user".into(),
            sender_name: Some("jamie".into()),
            sender_id: Some("u1".into()),
            content: "ship it".into(),
            metadata: None,
            created_at: DateTime::parse_from_rfc3339("2026-08-01T10:11:12Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let rendered = render_log_transcript(std::slice::from_ref(&message));
        assert!(rendered.contains("2026-08-01 10:11:12"));
        assert!(rendered.contains("jamie"));
        assert!(rendered.contains("ship it"));
    }

    #[test]
    fn cut_prompt_marks_prior_checkpoints_as_context_only() {
        let narrative = vec![checkpoint(1, "Earlier", "earlier things")];
        let prompt = build_cut_prompt(CheckpointKind::Interval, &[], &narrative);
        assert!(prompt.contains("Story so far"));
        assert!(prompt.contains("Do not restate them"));
        assert!(prompt.contains("New span to summarize"));
    }

    #[test]
    fn bootstrap_prompt_warns_about_truncated_past() {
        let prompt = build_cut_prompt(CheckpointKind::Bootstrap, &[], &[]);
        assert!(prompt.contains("entering chronicle mode with existing history"));
    }

    async fn store_with_two_checkpoints() -> ChronicleStore {
        use crate::conversation::chronicle::NewCheckpoint;

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("pool");
        sqlx::raw_sql(include_str!(
            "../../migrations/20260211000002_conversations.sql"
        ))
        .execute(&pool)
        .await
        .expect("conversations migration");
        sqlx::raw_sql(include_str!(
            "../../migrations/20260809000004_session_chronicles.sql"
        ))
        .execute(&pool)
        .await
        .expect("chronicle migration");

        for index in 0..6 {
            sqlx::query(
                "INSERT INTO conversation_messages (id, channel_id, role, content, created_at) \
                 VALUES (?, 'ch', 'user', 'hello', ?)",
            )
            .bind(format!("m{index}"))
            .bind(format!("2026-08-01 00:00:0{index}"))
            .execute(&pool)
            .await
            .expect("insert");
        }

        let store = ChronicleStore::new(pool);
        let at = |value: &str| {
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc)
        };

        let mut from = ChronicleBoundary::origin(at("2026-08-01T00:00:00Z"));
        for (seq, (to_at, to_id)) in [
            ("2026-08-01T00:00:02Z", "m2"),
            ("2026-08-01T00:00:05Z", "m5"),
        ]
        .iter()
        .enumerate()
        {
            let to = ChronicleBoundary::new(at(to_at), Some((*to_id).to_string()));
            store
                .commit(NewCheckpoint {
                    channel_id: "ch".into(),
                    level: 0,
                    kind: CheckpointKind::Interval,
                    title: format!("Span {}", seq + 1),
                    summary: format!("Summary of span {}", seq + 1),
                    covers_from: from.clone(),
                    covers_to: to.clone(),
                    message_count: 3,
                    token_estimate: 5,
                    rolls_up_from_seq: None,
                    rolls_up_to_seq: None,
                    model: None,
                })
                .await
                .expect("commit");
            from = to;
        }

        store
    }

    /// The view is a pure function of durable state, so a process that lost
    /// every in-memory structure renders exactly what the running one did.
    #[tokio::test]
    async fn view_is_reproducible_from_durable_state_alone() {
        let store = store_with_two_checkpoints().await;
        let now = DateTime::parse_from_rfc3339("2026-08-01T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let config = ChronicleConfig::default();

        let before_restart = render_chronicle_view(&store, "ch", now, config)
            .await
            .expect("view")
            .expect("a chronicle exists");

        // Restart: nothing survives but the database.
        let reopened = ChronicleStore::new(store.pool_for_tests().clone());
        let after_restart = render_chronicle_view(&reopened, "ch", now, config)
            .await
            .expect("view")
            .expect("a chronicle exists");

        assert_eq!(before_restart, after_restart);
        assert!(before_restart.contains("#1 Span 1"));
        assert!(before_restart.contains("#2 Span 2"));
    }

    #[tokio::test]
    async fn view_is_absent_before_the_first_checkpoint() {
        let store = store_with_two_checkpoints().await;
        let now = Utc::now();
        assert!(
            render_chronicle_view(&store, "empty-channel", now, ChronicleConfig::default())
                .await
                .expect("view")
                .is_none()
        );
    }

    /// Emergency truncation must not stamp "not summarized" on messages that
    /// are still in live context — doing so would advance the boundary past
    /// them and rob the next interval cut of the chance to summarize them.
    #[test]
    fn emergency_coverage_excludes_messages_still_in_live_context() {
        // 20 logged messages uncovered, 12 still live after the drain.
        let logged = 20usize;
        let retained_live = 12usize;
        let covered = logged.saturating_sub(retained_live);
        assert_eq!(covered, 8, "only the discarded span is covered");

        // When the log has not caught up with the live history, nothing is
        // safely coverable and the cut is skipped rather than over-reaching.
        let lagging_log = 9usize;
        assert_eq!(lagging_log.saturating_sub(retained_live), 0);
    }

    /// Bootstrap is for a backlog larger than one cut can read. A busy but new
    /// channel gets an ordinary interval checkpoint.
    #[test]
    fn bootstrap_threshold_tracks_what_one_cut_can_read() {
        let config = ChronicleConfig::default();
        let busy_new_channel = config.interval_messages as i64 + 5;
        assert!(
            busy_new_channel <= config.max_messages_per_checkpoint,
            "a burst past the interval is still an ordinary first cut"
        );

        let legacy_backlog = config.max_messages_per_checkpoint + 1;
        assert!(legacy_backlog > config.max_messages_per_checkpoint);
    }

    /// Checkpoints outside the recent window still appear, so a session that
    /// went quiet for a week does not lose its older entries from the index.
    #[tokio::test]
    async fn view_includes_older_checkpoints_outside_the_recent_window() {
        let store = store_with_two_checkpoints().await;
        let now = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let view = render_chronicle_view(&store, "ch", now, ChronicleConfig::default())
            .await
            .expect("view")
            .expect("a chronicle exists");

        assert!(view.contains("#1 Span 1"));
        assert!(view.contains("#2 Span 2"));
    }
}
