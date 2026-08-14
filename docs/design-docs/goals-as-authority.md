# Goals as Standing Authority

Spacebot's approval model is per-task: every task the agent proposes sits in
`pending_approval` until a human flips it to `ready`. That is a correct
default and a poor ceiling. It means autonomous work is only ever as broad
as the last thing Jamie individually authorized, and it puts him in the loop
once per unit of work rather than once per objective.

Goals are the missing coarse-grained grant. A goal is not a wish or a
milestone marker — **it is standing permission from a human to do what the
objective requires, until the objective is met.** "Release Spacebot 0.6" is
not a task; it is a scope of delegated authority that stays open across
weeks, under which the agent may derive work, prepare it, and — within
bounds — execute it, without asking again for each step.

This doc defines what that authority covers, what it can never cover, and
the rules governing how autonomy interacts with goals.

Related: [goals.md](goals.md) (data model, tools, injection),
[autonomy.md](autonomy.md) (levels), [autonomy-lifecycle.md](autonomy-lifecycle.md)
(deliberation), [human-scoped-turn-authority.md](human-scoped-turn-authority.md),
[autonomous-action-audit.md](autonomous-action-audit.md).

## Current state (source-grounded)

- Goals exist end to end — table, store, `goal_create`/`update`/`list`,
  API routes, a UI card, injection into channel and autonomy prompts — and
  **nothing reachable creates one.** `goal_create` is registered on
  conversation branches (`BranchToolProfile::Default`) and on the cortex
  chat toolset; channels and the autonomy channel get `goal_list` only. The
  interface client has `listGoals` and no `createGoal`; `GoalsCard` renders
  "No goals yet. Give your agent something to work toward" with no control
  that does so.
- The branch prompt never mentions `goal_create`. What it does teach is
  `MemoryType::Goal` — "something the user or agent wants to achieve" — a
  different concept with the same name. The overloaded word is why the tool
  is never called.
- `tasks.goal_id` exists and is settable, so the linkage the authority model
  needs is already in place.
- The autonomy prompt reads goals as "background context and direction, not
  a work queue," which is exactly the framing this doc changes.

## What a goal authorizes

A goal is a scope. Work falls inside it when a reasonable person would say
"yes, obviously, that's part of achieving this." Inside the scope, the agent
may act on the standing grant. Outside it, nothing changes — the ordinary
gates apply.

**Inside the grant:**

- **Derive work.** Create tasks linked to the goal (`goal_id` set) without
  being asked for each one.
- **Prepare.** Research, enrich, draft, stage. The release-notes case: if
  the goal is "release 0.6," drafting the notes is unambiguously in scope
  and unambiguously safe. Nobody needs to be asked whether release notes may
  be written for a release they asked for.
- **Order and re-order.** Set dependencies, decide sequence, decide what is
  blocked on what.
- **Execute reversible work** at the goal's authority level (below).
- **Report.** Update goal notes with progress, surface a completion
  candidate.

**Never inside the grant, regardless of goal or level:**

- **The terminal act.** The irreversible thing the goal is named after —
  cutting the release, sending the announcement, deleting the old system.
  A goal to release 0.6 authorizes everything up to the release and not the
  release. This is the sharp line: a goal authorizes *derivation and
  preparation*, never *consummation*.
- **Anything in the blast-radius rule** — spending money, messaging another
  human, destroying durable state. Goals do not launder these; they are
  confirmed with the human whether or not a goal covers the surrounding
  work.
- **Widening itself.** The agent may not edit a goal's title, description,
  or success condition to enlarge what it authorizes. Scope is set by the
  human, always.
- **Completing itself.** Unchanged from [goals.md](goals.md): the agent may
  mark a goal ready for review; the human closes it.

The asymmetry is deliberate. Preparation is cheap to undo and expensive to
delay; consummation is the reverse.

## Per-goal authority

How much a goal authorizes is a property of the goal, set by the human who
created it, not a global switch:

```
authority TEXT NOT NULL DEFAULT 'propose'
    -- 'propose'  : derive tasks, all land in pending_approval (today's behavior)
    -- 'prepare'  : derive + execute reversible preparatory work; anything
    --              that writes outside the workspace or touches a remote
    --              still needs approval
    -- 'execute'  : derive + execute the goal's ordinary work, including
    --              repo changes and PRs; terminal acts and blast-radius
    --              actions still gated
```

Default `propose` — a goal created without a stated authority changes
nothing about approval, only about priority and coherence. Raising it is an
explicit act by the human, per goal, and is the thing that actually buys
back their attention.

This composes with the autonomy level rather than replacing it: **the level
says how much the agent may do at all; the goal says what it may do it
toward.** Effective authority is the intersection — an `observe` agent with
an `execute` goal still only observes. A goal cannot exceed the level, and
the instance ceiling still caps both.

The task the agent derives records which authority admitted it
(`metadata.admitted_by: {goal_id, authority}`), so the audit trail answers
"why was this allowed to run without me?" — the question
[autonomous-action-audit.md](autonomous-action-audit.md) exists to answer.

## Success conditions

"Broad and present until the condition is met" requires the condition to be
written down. Goals gain:

```
success_condition TEXT
```

Prose, not a predicate — "0.6 is tagged, released, and the changelog is
published." Its job is to be evaluable by a model at deliberation time and
legible to a human at review time.

Autonomy evaluates progress against it and may report — "all linked tasks
complete, condition appears met, ready for your review" — but never closes
the goal. That rule is inherited from [goals.md](goals.md) and is
load-bearing here: a system that can both define what "done" means and
declare itself done has no external check.

An active goal with no unmet condition is the signal that the agent should
ask, not assume.

## Worked example: "Release Spacebot 0.6"

The goal Jamie states in conversation. Authority `prepare`. Condition: 0.6
tagged and released with published notes.

1. **Deliberation** ([autonomy-lifecycle.md](autonomy-lifecycle.md) §2) sees
   an active goal with no linked tasks and derives them: audit open PRs for
   release readiness; verify CI green on main; draft release notes; check
   migration safety.
2. Derived tasks link to the goal. Under `prepare`, the reversible ones —
   the audit, the CI check, the notes draft — are admitted without
   individual approval. The task that would push a tag is not: it is
   terminal, so it lands `pending_approval` no matter what the goal says.
3. Runs execute the admitted work over days. The release-notes task produces
   a `plan.md`-style attachment and a comment. The PR audit produces a
   finding: two PRs are not merge-ready, which becomes a task, which becomes
   a dependency edge on the release task.
4. When the standing PRs land, the next deliberation notices the blocked
   work is unblocked, and the release task — sitting in `pending_approval`
   with everything under it done — is the single decision left.
5. Jamie approves it. The goal's condition is met. Autonomy notes it as
   ready for review; Jamie closes the goal.

What he did: stated a goal, and approved one release. What he did not do:
approve fifteen preparatory tasks one at a time. That difference is the
entire point.

## Rules for autonomy

Autonomy's permitted interactions with goals, in full:

| Action | Allowed |
|---|---|
| Read goals, rank work by them | Yes |
| Create tasks linked to a goal | Yes |
| Execute goal-linked work | Per goal authority ∩ autonomy level |
| Update goal `notes` (progress) | Yes |
| Mark a goal ready for review | Yes |
| **Propose** a new goal | Yes — created `status: proposed`, inert until a human accepts |
| Create an `active` goal | No |
| Change a goal's title, description, condition, or authority | No |
| Complete or abandon a goal | No |

A proposed goal is a suggestion with no authority attached — it grants
nothing until a human accepts it, at which point they also set its
authority. This lets the agent say "you seem to be working toward X, should
that be a goal?" without the answer being self-granted permission.

## Creation from conversation

Goals originate in conversation, so `goal_create` belongs on the channel
toolset, not only behind a branch. A goal is a short, explicit statement of
intent — "I want to release 0.6" — and routing that through a branch adds
indirection with nothing to protect: the branch exists to keep memory
recall out of channel context, which does not apply here.

- Register `goal_create` and `goal_update` on the channel toolset;
  `goal_update` stays unable to touch scope fields per the table above.
- Teach the distinction in the prompt explicitly, because the current
  collision is why the tool is never called: **`goal_create` records a
  durable objective on the board and grants standing authority toward it;
  a `goal`-type memory is a private note about something the user wants.**
  When the user says what they want to achieve, the goal board is the
  correct home.
- Add `createGoal`/`updateGoal` to the API client and a create form to
  `GoalsCard`, which already advertises the empty state. Authority is set
  at creation in the UI, where the human is present to choose it.

## Failure modes

- **Scope creep by derivation.** The agent derives tasks that stretch the
  goal past what was meant. Mitigations: derived tasks record their
  admitting goal, deliberation states why a task serves the goal, and the
  human sees goal-linked work in the approval queue. `propose` remains the
  default precisely because scope judgment is the risky part.
- **Goal used to justify a blast-radius action.** Blocked structurally —
  those checks do not consult goals.
- **Stale goal grants authority forever.** Goals with no linked activity for
  a long window surface for review rather than expiring silently; an
  abandoned objective should be closed by a human, not time out into
  ambiguity.
- **Level/authority confusion.** Effective authority is always the
  intersection; the UI shows both so an `execute` goal on an `observe`
  agent reads as inert rather than dangerous.

## Phases

1. **Reachable creation.** `goal_create`/`goal_update` on the channel
   toolset, prompt text distinguishing goals from goal-memories,
   `createGoal` in the API client, create form on `GoalsCard`. Fixes the
   dead subsystem regardless of the rest.
2. **Scope fields.** `success_condition` and `authority` columns, set at
   creation, surfaced in prompt injection and UI.
3. **Admission.** Derived tasks record `goal_id` and their admitting
   authority; approval bypass for admitted reversible work; terminal-act and
   blast-radius gates verified to ignore goals.
4. **Proposed goals.** `status: proposed`, agent-side proposal, human
   acceptance sets authority.
5. **Condition evaluation.** Deliberation evaluates progress against
   `success_condition`, reports readiness, never closes.
