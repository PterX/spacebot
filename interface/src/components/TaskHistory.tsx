import {useCallback, useEffect, useRef, useState} from "react";
import {useMutation, useQuery, useQueryClient} from "@tanstack/react-query";
import {FontAwesomeIcon} from "@fortawesome/react-fontawesome";
import {
	faRobot,
	faUser,
	faGear,
	faCircleInfo,
	faClockRotateLeft,
} from "@fortawesome/free-solid-svg-icons";
import {Badge, Button} from "@spacedrive/primitives";
import {
	api,
	TaskRequestError,
	type TaskAuthorKind,
	type TaskRevisionSummary,
} from "@/api/client";
import {useLiveContext} from "@/hooks/useLiveContext";

const PAGE_SIZE = 50;

const AUTHOR_ICON: Record<TaskAuthorKind, typeof faUser> = {
	user: faUser,
	agent: faRobot,
	worker: faGear,
	system: faCircleInfo,
};

/** Human labels for the snapshot fields a diff can report. */
const FIELD_LABEL: Record<string, string> = {
	title: "Title",
	description: "Description",
	status: "Status",
	priority: "Priority",
	assigned_agent_id: "Assignee",
	subtasks: "Subtasks",
	metadata: "Metadata",
	goal_id: "Goal",
	worker_type: "Worker type",
	project_id: "Project",
	repo_id: "Repo",
	worktree_mode: "Worktree mode",
	worktree_id: "Worktree",
	required_skills: "Required skills",
	depends_on: "Dependencies",
};

function formatTimestamp(value: string): string {
	const parsed = new Date(value);
	return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString();
}

function renderValue(value: unknown): string {
	if (value === null || value === undefined) return "—";
	if (typeof value === "string") return value || "—";
	return JSON.stringify(value, null, 2);
}

/**
 * What a revision did, in one line.
 *
 * A restore says so explicitly — otherwise it reads like an ordinary edit that
 * happened to reproduce an old version, which is exactly the confusion history
 * is meant to remove.
 */
function revisionLabel(revision: TaskRevisionSummary): string {
	if (revision.restored_from != null) {
		return `Restored revision ${revision.restored_from}`;
	}
	return revision.edit_summary || "Edited";
}

function RevisionRow({
	revision,
	selected,
	onSelect,
	resolveAgentName,
}: {
	revision: TaskRevisionSummary;
	selected: boolean;
	onSelect: () => void;
	resolveAgentName?: (agentId: string) => string;
}) {
	const author =
		revision.author_type === "agent" && revision.author_id
			? (resolveAgentName?.(revision.author_id) ?? revision.author_id)
			: revision.author_type === "system"
				? "Spacebot"
				: (revision.author_id ?? revision.author_type);

	return (
		<li>
			<button
				type="button"
				onClick={onSelect}
				aria-pressed={selected}
				className={`w-full border-b border-app-line/40 px-1 py-2 text-left last:border-b-0 hover:bg-app-box/40 ${
					selected ? "bg-app-box/60" : ""
				}`}
			>
				<div className="flex items-center gap-2">
					<span className="font-mono text-[11px] text-ink-dull">
						r{revision.revision}
					</span>
					<Badge variant="default" size="sm">
						<FontAwesomeIcon
							icon={AUTHOR_ICON[revision.author_type]}
							className="text-[10px]"
						/>
						<span>{author}</span>
					</Badge>
					<span className="text-[10px] text-ink-faint">{revision.source}</span>
					<span className="ml-auto text-[10px] text-ink-faint">
						{formatTimestamp(revision.created_at)}
					</span>
				</div>
				<p className="mt-0.5 truncate text-xs text-ink-dull">
					{revisionLabel(revision)}
				</p>
			</button>
		</li>
	);
}

function DiffView({taskNumber, from, to}: {taskNumber: number; from: number; to: number}) {
	const {data, isLoading, error} = useQuery({
		queryKey: ["task-diff", taskNumber, from, to],
		queryFn: () => api.diffTaskRevisions(taskNumber, from, to),
	});

	if (isLoading) return <p className="text-xs text-ink-faint">Loading diff…</p>;
	if (error) return <p className="text-xs text-red-400">Failed to load the diff.</p>;
	if (!data?.changes.length) {
		return (
			<p className="text-xs text-ink-faint">
				Revisions {from} and {to} are materially identical.
			</p>
		);
	}

	return (
		<div className="space-y-2">
			{data.changes.map((change) => (
				<div key={change.field} className="rounded border border-app-line/60 p-2">
					<p className="mb-1 text-[11px] font-medium text-ink-dull">
						{FIELD_LABEL[change.field] ?? change.field}
					</p>
					<pre className="max-h-32 overflow-auto whitespace-pre-wrap break-words rounded bg-red-500/10 p-1.5 font-mono text-[11px] leading-relaxed text-ink-dull">
						{renderValue(change.before)}
					</pre>
					<pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap break-words rounded bg-green-500/10 p-1.5 font-mono text-[11px] leading-relaxed text-ink-dull">
						{renderValue(change.after)}
					</pre>
				</div>
			))}
		</div>
	);
}

function SnapshotView({taskNumber, revision}: {taskNumber: number; revision: number}) {
	const {data, isLoading, error} = useQuery({
		queryKey: ["task-revision", taskNumber, revision],
		queryFn: () => api.getTaskRevision(taskNumber, revision),
	});

	if (isLoading) return <p className="text-xs text-ink-faint">Loading revision…</p>;
	if (error) return <p className="text-xs text-red-400">Failed to load the revision.</p>;

	const snapshot = data?.revision.snapshot;
	if (!snapshot) return null;

	return (
		<dl className="space-y-1.5 text-xs">
			<div>
				<dt className="text-[10px] uppercase tracking-wide text-ink-faint">Title</dt>
				<dd className="text-ink-dull">{snapshot.title}</dd>
			</div>
			<div>
				<dt className="text-[10px] uppercase tracking-wide text-ink-faint">Status</dt>
				<dd className="text-ink-dull">
					{snapshot.status} · {snapshot.priority}
				</dd>
			</div>
			<div>
				<dt className="text-[10px] uppercase tracking-wide text-ink-faint">
					Description
				</dt>
				<dd className="whitespace-pre-wrap break-words text-ink-dull">
					{snapshot.description || "—"}
				</dd>
			</div>
			{snapshot.subtasks.length > 0 && (
				<div>
					<dt className="text-[10px] uppercase tracking-wide text-ink-faint">
						Subtasks
					</dt>
					<dd>
						<ul className="text-ink-dull">
							{snapshot.subtasks.map((subtask, index) => (
								<li key={`${subtask.title}-${index}`}>
									[{subtask.completed ? "x" : " "}] {subtask.title}
								</li>
							))}
						</ul>
					</dd>
				</div>
			)}
		</dl>
	);
}

/**
 * Revision history for a task: what changed, when, by whom, and the way back.
 *
 * Restore is confirmed and requires a summary, and it carries the revision the
 * user was looking at — if the task moved on while the dialog was open, the
 * server rejects it and the conflict is shown rather than silently applied.
 */
export function TaskHistory({
	taskNumber,
	currentRevision,
	resolveAgentName,
}: {
	taskNumber: number;
	currentRevision: number;
	resolveAgentName?: (agentId: string) => string;
}) {
	const queryClient = useQueryClient();
	const {taskRevisionVersion} = useLiveContext();
	const queryKey = ["task-revisions", taskNumber];

	const [selected, setSelected] = useState<number | null>(null);
	const [mode, setMode] = useState<"diff" | "snapshot">("diff");
	const [confirming, setConfirming] = useState(false);
	const [summary, setSummary] = useState("");

	const previousVersion = useRef(taskRevisionVersion);
	useEffect(() => {
		if (taskRevisionVersion !== previousVersion.current) {
			previousVersion.current = taskRevisionVersion;
			void queryClient.invalidateQueries({queryKey});
		}
	}, [taskRevisionVersion, queryClient, taskNumber]);

	// A different task in the detail pane starts from a clean slate.
	useEffect(() => {
		setSelected(null);
		setConfirming(false);
		setSummary("");
	}, [taskNumber]);

	const {data, isLoading, error} = useQuery({
		queryKey,
		queryFn: () => api.listTaskRevisions(taskNumber, PAGE_SIZE),
	});

	const restoreMutation = useMutation({
		mutationFn: (revision: number) =>
			api.restoreTaskRevision(taskNumber, revision, {
				expected_revision: currentRevision,
				edit_summary: summary.trim() || undefined,
			}),
		onSuccess: () => {
			setConfirming(false);
			setSummary("");
			setSelected(null);
			void queryClient.invalidateQueries({queryKey});
			void queryClient.invalidateQueries({queryKey: ["tasks"]});
		},
	});

	const handleRestore = useCallback(() => {
		if (selected == null || !summary.trim()) return;
		restoreMutation.mutate(selected);
	}, [selected, summary, restoreMutation]);

	const revisions = data?.revisions ?? [];
	const restoreError = restoreMutation.error as TaskRequestError | Error | undefined;
	const isConflict =
		restoreError instanceof TaskRequestError && restoreError.isConflict;

	return (
		<div className="border-t border-app-line/40 px-4 py-3">
			<h3 className="mb-2 flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-ink-dull">
				<FontAwesomeIcon icon={faClockRotateLeft} className="text-[10px]" />
				History
				{data ? <span className="text-ink-faint">· r{data.current}</span> : null}
			</h3>

			{isLoading ? (
				<p className="text-xs text-ink-faint">Loading history…</p>
			) : error ? (
				<p className="text-xs text-red-400">Failed to load history.</p>
			) : revisions.length === 0 ? (
				<p className="text-xs text-ink-faint">
					No revisions recorded for this task yet.
				</p>
			) : (
				<ul>
					{revisions.map((revision) => (
						<RevisionRow
							key={revision.id}
							revision={revision}
							selected={selected === revision.revision}
							onSelect={() =>
								setSelected((current) =>
									current === revision.revision ? null : revision.revision,
								)
							}
							resolveAgentName={resolveAgentName}
						/>
					))}
				</ul>
			)}

			{selected != null && (
				<div className="mt-3 border-t border-app-line/40 pt-3">
					<div className="mb-2 flex items-center gap-2">
						<Button
							size="sm"
							variant={mode === "diff" ? "accent" : "gray"}
							onClick={() => setMode("diff")}
						>
							Diff vs current
						</Button>
						<Button
							size="sm"
							variant={mode === "snapshot" ? "accent" : "gray"}
							onClick={() => setMode("snapshot")}
						>
							Snapshot
						</Button>
					</div>

					{mode === "diff" ? (
						<DiffView taskNumber={taskNumber} from={selected} to={currentRevision} />
					) : (
						<SnapshotView taskNumber={taskNumber} revision={selected} />
					)}

					{selected !== currentRevision && (
						<div className="mt-3">
							{confirming ? (
								<>
									<p className="mb-1.5 text-xs text-ink-dull">
										Restore revision {selected}? This appends a new revision —
										nothing in the history is erased.
									</p>
									<input
										value={summary}
										onChange={(event) => setSummary(event.target.value)}
										placeholder="Why are you restoring this?"
										aria-label="Reason for restoring"
										className="w-full rounded border border-app-line bg-app-input px-2 py-1.5 text-xs text-ink placeholder:text-ink-faint focus:border-accent focus:outline-none"
									/>
									<div className="mt-1.5 flex items-center gap-2">
										<Button
											size="sm"
											disabled={!summary.trim() || restoreMutation.isPending}
											onClick={handleRestore}
										>
											{restoreMutation.isPending ? "Restoring…" : "Restore"}
										</Button>
										<Button
											size="sm"
											variant="gray"
											onClick={() => {
												setConfirming(false);
												restoreMutation.reset();
											}}
										>
											Cancel
										</Button>
									</div>
								</>
							) : (
								<Button size="sm" variant="gray" onClick={() => setConfirming(true)}>
									Restore revision {selected}
								</Button>
							)}

							{restoreError && (
								<p className="mt-1.5 text-[11px] text-red-400">
									{isConflict
										? `This task changed while you were looking — it is now at revision ${
												(restoreError as TaskRequestError).currentRevision
											}. Reload and try again.`
										: restoreError.message}
								</p>
							)}
						</div>
					)}
				</div>
			)}
		</div>
	);
}
