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

export interface AutonomyRunSummary {
	id: string;
	started_at: string;
	duration_secs: number;
	summary: string;
	actions: AutonomyRunAction[];
}

export interface PendingBrief {
	id: string;
	title: string;
	finding: string;
	comment_count: number;
	worker_count: number;
	goal_title: string | null;
	enriched_at: string | null;
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
		started_at: minutesAgo(23),
		duration_secs: 372,
		summary: "Enriched 2 tasks, proposed 1 new task",
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
		started_at: minutesAgo(53),
		duration_secs: 124,
		summary: "Nothing needed attention",
		actions: [],
	},
	{
		id: "run-3",
		started_at: minutesAgo(83),
		duration_secs: 527,
		summary: "Executed 1 approved task, proposed 1 new task",
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
		started_at: minutesAgo(113),
		duration_secs: 240,
		summary: "Enriched 1 task",
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
		started_at: minutesAgo(143),
		duration_secs: 98,
		summary: "Nothing to do",
		actions: [],
	},
];

const pending: PendingBrief[] = [
	{
		id: "task-1",
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

export const mockAutonomyApi = {
	status: (): Promise<AutonomyStatus> => Promise.resolve(status),
	runs: (): Promise<{runs: AutonomyRunSummary[]}> => Promise.resolve({runs}),
	pending: (): Promise<{tasks: PendingBrief[]}> => Promise.resolve({tasks: pending}),
	goals: (): Promise<{goals: AutonomyGoal[]}> => Promise.resolve({goals}),
};
