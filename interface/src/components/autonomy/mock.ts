// Mock data for the autonomy panel. Shaped like the future /api/autonomy
// surface so wiring the real backend is a swap of this module for api/client
// methods, not a rewrite of the components.

export type AutonomyLevel = "off" | "observe" | "suggest" | "act";

export interface AutonomyStatus {
	level: AutonomyLevel;
	interval_secs: number;
	active_hours: [number, number] | null;
	max_tasks_per_run: number;
	last_run_at: string | null;
	next_run_at: string | null;
	current_run: CurrentRun | null;
}

export interface CurrentRun {
	started_at: string;
	activity: string;
}

export interface AutonomyRunAction {
	kind: "enriched" | "created" | "executed";
	task_title: string;
	detail: string;
}

export type WakeTriggerKind = "schedule" | "webhook" | "event" | "condition";

export interface WokenBy {
	kind: WakeTriggerKind;
	label: string;
}

export interface AutonomyRunSummary {
	id: string;
	started_at: string;
	duration_secs: number;
	summary: string;
	agent_index: number;
	woken_by: WokenBy[];
	actions: AutonomyRunAction[];
}

export interface AgentAutonomyState {
	level: AutonomyLevel;
	last_run_at: string | null;
	last_summary: string | null;
	next_run_at: string | null;
	current_run: CurrentRun | null;
	pending_count: number;
}

export interface WakeDef {
	id: string;
	name: string;
	trigger_kind: WakeTriggerKind;
	trigger_label: string;
	instructions: string;
	min_level: Exclude<AutonomyLevel, "off">;
	builtin: boolean;
	enabled: boolean;
	last_fired_at: string | null;
}

export interface PendingBrief {
	id: string;
	title: string;
	finding: string;
	comment_count: number;
	worker_count: number;
	goal_title: string | null;
	enriched_at: string | null;
	agent_index: number;
}

export interface AutonomyGoal {
	id: string;
	title: string;
	priority: "critical" | "high" | "medium" | "low";
	notes: string;
	tasks_done: number;
	tasks_total: number;
	due_date: string | null;
}

function minutesAgo(n: number): string {
	return new Date(Date.now() - n * 60_000).toISOString();
}

function minutesFromNow(n: number): string {
	return new Date(Date.now() + n * 60_000).toISOString();
}

const status: AutonomyStatus = {
	level: "suggest",
	interval_secs: 1800,
	active_hours: [8, 22],
	max_tasks_per_run: 2,
	last_run_at: minutesAgo(23),
	next_run_at: minutesFromNow(7),
	current_run: null,
};

const runs: AutonomyRunSummary[] = [
	{
		id: "run-1",
		agent_index: 0,
		started_at: minutesAgo(23),
		duration_secs: 372,
		summary: "Enriched 2 tasks, proposed 1 new task",
		woken_by: [
			{kind: "schedule", label: "Idle survey"},
			{kind: "event", label: "Comment on: Fix Telegram adapter reconnect loop"},
		],
		actions: [
			{
				kind: "enriched",
				task_title: "Fix Telegram adapter reconnect loop",
				detail:
					"2 investigation workers · root cause found in retry backoff, fix proposed",
			},
			{
				kind: "enriched",
				task_title: "Prune stale memories (34 candidates)",
				detail: "scanned recall logs, full candidate list posted with reasons",
			},
			{
				kind: "created",
				task_title: "Write weekly changelog draft",
				detail: "proposed from goal: Ship v0.6 with autonomy",
			},
		],
	},
	{
		id: "run-2",
		agent_index: 1,
		started_at: minutesAgo(53),
		duration_secs: 124,
		summary: "Nothing needed attention",
		woken_by: [{kind: "schedule", label: "Idle survey"}],
		actions: [],
	},
	{
		id: "run-3",
		agent_index: 0,
		started_at: minutesAgo(83),
		duration_secs: 527,
		summary: "Executed 1 approved task, proposed 1 new task",
		woken_by: [
			{kind: "event", label: "Approved: Rotate expiring webhook secrets"},
		],
		actions: [
			{
				kind: "executed",
				task_title: "Rotate expiring webhook secrets",
				detail: "completed · 2 secrets rotated, config hot-reloaded",
			},
			{
				kind: "created",
				task_title: "Prune stale memories (34 candidates)",
				detail: "proposed from memory audit during execution",
			},
		],
	},
	{
		id: "run-4",
		agent_index: 1,
		started_at: minutesAgo(113),
		duration_secs: 240,
		summary: "Enriched 1 task",
		woken_by: [{kind: "condition", label: "Quiet hours (2h idle)"}],
		actions: [
			{
				kind: "enriched",
				task_title: "Fix Telegram adapter reconnect loop",
				detail: "reproduced the failure in sandbox, narrowed to Bot API 429 handling",
			},
		],
	},
	{
		id: "run-5",
		agent_index: 0,
		started_at: minutesAgo(143),
		duration_secs: 98,
		summary: "Nothing to do",
		woken_by: [{kind: "schedule", label: "Idle survey"}],
		actions: [],
	},
];

const wakes: WakeDef[] = [
	{
		id: "wake-1",
		name: "Idle survey",
		trigger_kind: "schedule",
		trigger_label: "every 30m",
		instructions:
			"Survey task state, enrich pending proposals, execute approved work.",
		min_level: "observe",
		builtin: true,
		enabled: true,
		last_fired_at: minutesAgo(23),
	},
	{
		id: "wake-2",
		name: "Task approved",
		trigger_kind: "event",
		trigger_label: "on approval",
		instructions: "Start approved work immediately instead of waiting for the next survey.",
		min_level: "act",
		builtin: true,
		enabled: true,
		last_fired_at: minutesAgo(83),
	},
	{
		id: "wake-3",
		name: "Morning brief",
		trigger_kind: "schedule",
		trigger_label: "daily at 8:00",
		instructions:
			"Summarize overnight activity and what needs my attention today. Deliver to my DM.",
		min_level: "observe",
		builtin: false,
		enabled: true,
		last_fired_at: minutesAgo(457),
	},
	{
		id: "wake-4",
		name: "CI failed on main",
		trigger_kind: "webhook",
		trigger_label: "POST /hooks/ci",
		instructions:
			"Investigate the failing job in the payload and propose a fix task with findings.",
		min_level: "suggest",
		builtin: false,
		enabled: true,
		last_fired_at: minutesAgo(2880),
	},
	{
		id: "wake-5",
		name: "Quiet-hours enrichment",
		trigger_kind: "condition",
		trigger_label: "no activity for 2h",
		instructions: "The humans are away. Use the time to research pending proposals.",
		min_level: "suggest",
		builtin: false,
		enabled: false,
		last_fired_at: minutesAgo(113),
	},
];

const pending: PendingBrief[] = [
	{
		id: "task-1",
		agent_index: 0,
		title: "Fix Telegram adapter reconnect loop",
		finding:
			"Reproduced: reconnect storms after a 429 from the Bot API. The retry path ignores retry_after and hammers the endpoint. Proposed fix is a ~30 line change to the adapter backoff; the Discord adapter has the same pattern.",
		comment_count: 4,
		worker_count: 2,
		goal_title: "Keep the agent reliable",
		enriched_at: minutesAgo(23),
	},
	{
		id: "task-2",
		agent_index: 1,
		title: "Write weekly changelog draft",
		finding:
			"Collected 14 merged PRs since Monday, grouped into 4 themes with highlights. Draft is in the comments — needs your voice pass before it goes anywhere.",
		comment_count: 2,
		worker_count: 1,
		goal_title: "Ship v0.6 with autonomy",
		enriched_at: minutesAgo(23),
	},
	{
		id: "task-3",
		agent_index: 0,
		title: "Prune stale memories (34 candidates)",
		finding:
			"Found 34 memories not recalled in 90+ days; 12 reference channels that no longer exist. Full list with per-memory reasons is in the comments. Deletion is reversible for 30 days.",
		comment_count: 1,
		worker_count: 1,
		goal_title: null,
		enriched_at: minutesAgo(83),
	},
];

const goals: AutonomyGoal[] = [
	{
		id: "goal-1",
		title: "Ship v0.6 with autonomy",
		priority: "high",
		notes: "Panel design in progress; core loop next. Changelog draft proposed.",
		tasks_done: 3,
		tasks_total: 8,
		due_date: "2026-08-29",
	},
	{
		id: "goal-2",
		title: "Keep the agent reliable",
		priority: "critical",
		notes: "Two adapter bugs found this week — one fixed, one investigated.",
		tasks_done: 5,
		tasks_total: 7,
		due_date: null,
	},
	{
		id: "goal-3",
		title: "Grow the community",
		priority: "low",
		notes: "Discord invite revamp proposed, waiting on review.",
		tasks_done: 1,
		tasks_total: 4,
		due_date: null,
	},
];

const fleet: AgentAutonomyState[] = [
	{
		level: "act",
		last_run_at: minutesAgo(23),
		last_summary: "Enriched 2 tasks · proposed 1",
		next_run_at: minutesFromNow(7),
		current_run: null,
		pending_count: 2,
	},
	{
		level: "suggest",
		last_run_at: minutesAgo(41),
		last_summary: "Proposed 1 task",
		next_run_at: minutesFromNow(19),
		current_run: {
			started_at: minutesAgo(2),
			activity: "Researching: Prune stale memories (34 candidates)",
		},
		pending_count: 1,
	},
];

export const mockAutonomyApi = {
	status: (): Promise<AutonomyStatus> => Promise.resolve(status),
	runs: (): Promise<{runs: AutonomyRunSummary[]}> => Promise.resolve({runs}),
	pending: (): Promise<{tasks: PendingBrief[]}> => Promise.resolve({tasks: pending}),
	goals: (): Promise<{goals: AutonomyGoal[]}> => Promise.resolve({goals}),
	wakes: (): Promise<{wakes: WakeDef[]}> => Promise.resolve({wakes}),
	fleet: (): Promise<{states: AgentAutonomyState[]}> =>
		Promise.resolve({states: fleet}),
};
