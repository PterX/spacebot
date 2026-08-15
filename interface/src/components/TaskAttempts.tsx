import {useEffect, useRef, useState} from "react";
import {useQuery, useQueryClient} from "@tanstack/react-query";
import {FontAwesomeIcon} from "@fortawesome/react-fontawesome";
import {
	faCheck,
	faChevronDown,
	faChevronRight,
	faCircleHalfStroke,
	faHourglassEnd,
	faPlug,
	faSpinner,
	faStop,
	faTriangleExclamation,
	faXmark,
} from "@fortawesome/free-solid-svg-icons";
import {Badge} from "@spacedrive/primitives";
import {api, type TaskAttempt, type TaskAttemptOutcome} from "@/api/client";
import {useLiveContext} from "@/hooks/useLiveContext";

type BadgeVariant = "info" | "success" | "warning" | "error" | "default";

const OUTCOME_LABEL: Record<TaskAttemptOutcome, string> = {
	succeeded: "Succeeded",
	partial: "Partial",
	blocked: "Blocked",
	failed: "Failed",
	cancelled: "Cancelled",
	timed_out: "Timed out",
	interrupted: "Interrupted",
};

const OUTCOME_ICON: Record<TaskAttemptOutcome, typeof faCheck> = {
	succeeded: faCheck,
	partial: faCircleHalfStroke,
	blocked: faTriangleExclamation,
	failed: faXmark,
	cancelled: faStop,
	timed_out: faHourglassEnd,
	interrupted: faPlug,
};

const OUTCOME_VARIANT: Record<TaskAttemptOutcome, BadgeVariant> = {
	succeeded: "success",
	partial: "info",
	blocked: "warning",
	failed: "error",
	cancelled: "default",
	timed_out: "warning",
	interrupted: "default",
};

function formatTimestamp(value: string): string {
	const parsed = new Date(value);
	return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString();
}

/** Wall-clock duration, or how long a live run has been going. */
function formatDuration(startedAt: string, endedAt?: string | null): string | null {
	const start = new Date(startedAt).getTime();
	const end = endedAt ? new Date(endedAt).getTime() : Date.now();
	if (Number.isNaN(start) || Number.isNaN(end) || end < start) return null;

	const seconds = Math.round((end - start) / 1000);
	if (seconds < 60) return `${seconds}s`;
	const minutes = Math.floor(seconds / 60);
	if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
	return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/**
 * The run's own output, fetched only when asked for.
 *
 * The attempt row records how the run ended; the worker holds what it actually
 * produced, and that is often long enough to bury everything else.
 */
function AttemptOutput({agentId, workerId}: {agentId: string; workerId: string}) {
	const [expanded, setExpanded] = useState(false);

	const {data, isLoading, error} = useQuery({
		queryKey: ["worker-detail", agentId, workerId],
		queryFn: () => api.workerDetail(agentId, workerId),
		enabled: expanded,
		staleTime: 60_000,
	});

	return (
		<div className="mt-1.5">
			<button
				type="button"
				onClick={() => setExpanded((open) => !open)}
				aria-expanded={expanded}
				className="inline-flex items-center gap-1.5 text-[11px] text-ink-dull hover:text-ink"
			>
				<FontAwesomeIcon
					icon={expanded ? faChevronDown : faChevronRight}
					className="text-[9px]"
				/>
				<span>Worker output</span>
			</button>

			{expanded && (
				<div className="mt-1.5 rounded border border-app-line/60 bg-app-box/40 p-2">
					{isLoading ? (
						<span className="text-[11px] text-ink-faint">Loading worker output…</span>
					) : error ? (
						<span className="text-[11px] text-red-400">
							Worker run is no longer available.
						</span>
					) : (
						<pre className="max-h-60 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-ink-dull">
							{data?.result?.trim() || "This worker recorded no output."}
						</pre>
					)}
				</div>
			)}
		</div>
	);
}

function AttemptRow({attempt, agentId}: {attempt: TaskAttempt; agentId?: string}) {
	const live = !attempt.ended_at;
	const outcome = attempt.outcome ?? null;
	const duration = formatDuration(attempt.started_at, attempt.ended_at);

	return (
		<li className="border-b border-app-line/40 py-2.5 last:border-b-0">
			<div className="mb-1 flex flex-wrap items-center gap-2">
				<span className="font-mono text-[11px] text-ink-faint">#{attempt.attempt}</span>

				{live ? (
					<Badge variant="info" size="sm">
						<FontAwesomeIcon icon={faSpinner} className="animate-spin text-[10px]" />
						<span>Running</span>
					</Badge>
				) : outcome ? (
					<Badge variant={OUTCOME_VARIANT[outcome]} size="sm">
						<FontAwesomeIcon icon={OUTCOME_ICON[outcome]} className="text-[10px]" />
						<span>{OUTCOME_LABEL[outcome]}</span>
					</Badge>
				) : (
					<Badge variant="default" size="sm">
						<span>Ended without an outcome</span>
					</Badge>
				)}

				<span className="font-mono text-[10px] text-ink-faint">
					{attempt.worker_id.slice(0, 8)}
				</span>
				<span className="text-[10px] text-ink-faint">
					{formatTimestamp(attempt.started_at)}
					{duration ? ` · ${duration}` : ""}
				</span>
				{attempt.channel_id && (
					<span className="text-[10px] text-ink-faint">via {attempt.channel_id}</span>
				)}
			</div>

			{attempt.outcome_summary && (
				<p className="whitespace-pre-wrap break-words text-xs leading-relaxed text-ink-dull">
					{attempt.outcome_summary}
				</p>
			)}

			{agentId && <AttemptOutput agentId={agentId} workerId={attempt.worker_id} />}
		</li>
	);
}

/**
 * Every worker run attempted against this task.
 *
 * The task row names only the run executing now, so without this a task that
 * failed twice before succeeding looks identical to one that worked first time.
 */
export function TaskAttempts({
	taskNumber,
	agentId,
}: {
	taskNumber: number;
	agentId?: string;
}) {
	const queryClient = useQueryClient();
	const {workerEventVersion} = useLiveContext();
	const queryKey = ["task-attempts", taskNumber];

	// A run starting or finishing arrives over SSE.
	const previousVersion = useRef(workerEventVersion);
	useEffect(() => {
		if (workerEventVersion !== previousVersion.current) {
			previousVersion.current = workerEventVersion;
			void queryClient.invalidateQueries({queryKey});
		}
	}, [workerEventVersion, queryClient, taskNumber]);

	const {data, isLoading, error} = useQuery({
		queryKey,
		queryFn: () => api.listTaskAttempts(taskNumber),
	});

	const attempts = data?.attempts ?? [];

	return (
		<div className="border-t border-app-line/40 px-4 py-3">
			<h3 className="mb-2 text-xs font-medium uppercase tracking-wide text-ink-dull">
				Runs{attempts.length > 0 ? ` (${attempts.length})` : ""}
			</h3>

			{isLoading ? (
				<p className="text-xs text-ink-faint">Loading runs…</p>
			) : error ? (
				<p className="text-xs text-red-400">Failed to load the run history.</p>
			) : attempts.length === 0 ? (
				<p className="text-xs text-ink-faint">
					Not worked yet. Every worker run against this task is recorded here.
				</p>
			) : (
				<>
					{data?.summary && (
						<p className="mb-2 text-[11px] text-ink-faint">{data.summary}</p>
					)}
					<ul>
						{attempts.map((attempt) => (
							<AttemptRow key={attempt.id} attempt={attempt} agentId={agentId} />
						))}
					</ul>
				</>
			)}
		</div>
	);
}
