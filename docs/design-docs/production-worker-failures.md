# Historical Worker Failure Analysis

This records production incidents observed in February 2026. It is not a
description of current worker behavior. The raw logs behind the quoted errors
and aggregate counts are not retained in this repository.

The incidents exposed a bad reliability model: the runtime treated context
pressure, provider faults, elapsed time, and supervision gaps as reasons to
end a worker. A worker owns a task until it reaches a durable outcome. Guards
must preserve that ownership, checkpoint its work, and allow it to continue or
resume.

The durable execution plan is in
[`durable-worker-execution.md`](durable-worker-execution.md). Existing
lifecycle work is in [`worker-reliability.md`](worker-reliability.md) and
[`worker-lifecycle-convergence.md`](worker-lifecycle-convergence.md).

## Incidents Observed

### Context Length Exceeded

Workers accumulated shell output, directory listings, search results, and file
reads until provider context limits rejected the next request. One request
contained approximately 1.9 million tokens against a 262,144 token limit.

At the time, tool results entered history without a size limit and workers had
no context measurement or recovery path. A worker investigating SQL searched
10,666 TypeScript files, including dependency and build directories, then
continued reading files instead of reporting the matches it had already found.

### Provider Empty Responses and Response Decode Errors

Providers sometimes returned an empty response or failed while decoding a
partially read response body. The worker retried the same request and then
ended after its retry allowance was exhausted.

The recorded sample reported roughly 10 empty-response failures over 24 hours
and 8 response-decoding failures over 48 hours. Those counts are retained as
historical notes only.

### Unbounded Tool Results

A shell `find` traversed `node_modules` and returned more than 5,000 paths.
The complete result was serialized into the worker history. Other incidents
included recursive searches with tens of kilobytes of match output and large
schema file reads.

### Environment Assumptions

Workers assumed Linux paths while running on macOS, used literal `~` paths in
file operations, and retried malformed shell or CLI commands. Some commands
printed usage errors while returning exit code zero, which made them appear
successful to the worker.

### Browser Startup and Access Failures

Chromium startup failed when a stale `ProcessSingleton` lock remained. Workers
then fell back to shell commands that did not provide the requested structured
data. Login walls, CAPTCHA pages, and WAF responses created similar dead ends.

### Investigation Without a Handoff

Some workers kept gathering evidence after they had enough to answer the task.
One recorded worker reached 47 messages without reporting a conclusion or
terminal outcome. The failure was not the amount of investigation. It was that
the runtime had no durable checkpoint, progress-aware steering, or safe
continuation path.

## What Has Changed

The following mitigations now exist:

- Shell and file tool output is capped at 50 KB. Directory listings are capped
  at 500 entries.
- Workers estimate history size, compact before calls at 70% of the configured
  context window, and attempt bounded overflow recovery.
- Empty responses and response-body decoding failures are retriable. Routed
  completion calls use backoff and configured fallback models.
- Text-only worker completion requires a terminal outcome signal.
- Browser startup removes stale Chromium singleton locks. Browser block
  detection returns structured blocked outcomes for common login, CAPTCHA, and
  WAF pages.
- Terminal worker outcomes use a durable lifecycle transition and are committed
  before completion notifications.

These are mitigations, not the final reliability contract. The 50 KB cap can
still consume a large context budget. Active workers are still reconciled as
failed after a process restart. Retry, segment, overflow, inactivity, and
wall-clock ceilings can still stop a task rather than preserving its execution
state and resuming it.

## Lasting Lessons

1. Cap individual artifacts before they enter model context. Do not use a cap
   as a reason to abandon the task.
2. Treat context overflow as a checkpoint-and-continue event.
3. Treat transient provider and transport errors as retryable execution events.
4. A timeout or stalled-progress signal requests cooperative checkpointing. It
   is not a terminal outcome by itself.
5. Persist task ownership, transcript checkpoints, provider session metadata,
   and the next executable continuation before each external boundary.
6. Make active workers inspectable from durable state after restart, not only
   through live in-memory events.
7. Human cancellation and explicit external blocks remain terminal. Runtime
   guardrails must otherwise preserve the worker's ability to complete.
