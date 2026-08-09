import {useEffect, useState} from "react";
import {CaretDown, CaretRight} from "@phosphor-icons/react";
import {Card, CardContent, FilterButton} from "@spacedrive/primitives";
import type {AutonomyStatus, AutonomyUpdate} from "@/api/client";
import {LEVELS, LevelDial} from "./levels";

const INTERVAL_OPTIONS: {label: string; secs: number}[] = [
	{label: "15m", secs: 900},
	{label: "30m", secs: 1800},
	{label: "1h", secs: 3600},
	{label: "2h", secs: 7200},
];

const MAX_TASK_OPTIONS = [1, 2, 3, 5];

const HOUR_OPTIONS: {label: string; value: [number, number] | null}[] = [
	{label: "Always", value: null},
	{label: "8:00–22:00", value: [8, 22]},
	{label: "9:00–17:00", value: [9, 17]},
];

function useNow(intervalMs: number): number {
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		const id = setInterval(() => setNow(Date.now()), intervalMs);
		return () => clearInterval(id);
	}, [intervalMs]);
	return now;
}

function formatTimeAgo(iso: string, now: number): string {
	const seconds = Math.floor((now - new Date(iso).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}

function formatCountdown(iso: string, now: number): string {
	const seconds = Math.max(0, Math.floor((new Date(iso).getTime() - now) / 1000));
	const m = Math.floor(seconds / 60);
	const s = seconds % 60;
	return `${m}:${String(s).padStart(2, "0")}`;
}

function formatInterval(secs: number): string {
	if (secs < 3600) return `${Math.round(secs / 60)}m`;
	return `${Math.round(secs / 3600)}h`;
}

interface AutonomyDialCardProps {
	status: AutonomyStatus | undefined;
	onUpdate: (update: AutonomyUpdate) => void;
	agentName?: string;
}

export function AutonomyDialCard({status, onUpdate, agentName}: AutonomyDialCardProps) {
	const [advancedOpen, setAdvancedOpen] = useState(false);
	const now = useNow(1000);

	const level = status?.level ?? "off";
	const intervalSecs = status?.interval_secs ?? 1800;
	const maxTasks = status?.max_tasks_per_run ?? 2;
	const activeHours = status?.active_hours ?? null;

	const selected = LEVELS.find((l) => l.key === level) ?? LEVELS[0];
	const isOff = level === "off";

	return (
		<Card variant="dark">
			<CardContent className="p-6">
				<div className="mb-5 flex items-start justify-between">
					<div>
						<h1 className="font-plex text-lg font-semibold text-ink">
							Autonomy
						</h1>
						<p className="mt-0.5 text-sm text-ink-faint">
							{agentName
								? `How active ${agentName} is when you're not around.`
								: "How active your agent is when you're not around."}
						</p>
					</div>
				</div>

				<LevelDial value={level} onChange={(next) => onUpdate({level: next})} />

				<p className="mt-3 min-h-[2.5rem] text-sm text-ink-dull">
					{selected.tagline}
				</p>

				{/* Pulse row */}
				<div className="mt-4 grid grid-cols-3 gap-2">
					<div className="rounded-xl bg-app-box/60 px-4 py-3">
						<p className="text-tiny font-medium uppercase tracking-wide text-ink-faint">
							Last run
						</p>
						{status?.last_run_at ? (
							<>
								<p className="mt-1 font-plex text-xl font-semibold tabular-nums text-ink">
									{formatTimeAgo(status.last_run_at, now)}
								</p>
								<p className="mt-0.5 truncate text-tiny text-ink-faint">
									{status.last_run_summary ?? "no summary recorded"}
								</p>
							</>
						) : (
							<p className="mt-1 font-plex text-xl font-semibold text-ink-faint">
								—
							</p>
						)}
					</div>

					<div className="rounded-xl bg-app-box/60 px-4 py-3">
						<p className="text-tiny font-medium uppercase tracking-wide text-ink-faint">
							Next run
						</p>
						{!isOff && status?.next_run_at ? (
							<>
								<p className="mt-1 font-plex text-xl font-semibold tabular-nums text-ink">
									{formatCountdown(status.next_run_at, now)}
								</p>
								<p className="mt-0.5 truncate text-tiny text-ink-faint">
									every {formatInterval(intervalSecs)}
									{activeHours
										? ` · active ${activeHours[0]}:00–${activeHours[1]}:00`
										: ""}
								</p>
							</>
						) : (
							<>
								<p className="mt-1 font-plex text-xl font-semibold text-ink-faint">
									—
								</p>
								<p className="mt-0.5 text-tiny text-ink-faint">
									autonomy is off
								</p>
							</>
						)}
					</div>

					<div className="rounded-xl bg-app-box/60 px-4 py-3">
						<p className="text-tiny font-medium uppercase tracking-wide text-ink-faint">
							Right now
						</p>
						{isOff ? (
							<>
								<p className="mt-1 font-plex text-xl font-semibold text-ink-faint">
									Paused
								</p>
								<p className="mt-0.5 text-tiny text-ink-faint">
									responds only when spoken to
								</p>
							</>
						) : status?.current_run ? (
							<>
								<p className="mt-1 flex items-center gap-2 font-plex text-xl font-semibold text-ink">
									<span className="relative flex size-2.5">
										<span className="absolute inline-flex size-full animate-ping rounded-full bg-status-success opacity-60" />
										<span className="relative inline-flex size-2.5 rounded-full bg-status-success" />
									</span>
									Working
								</p>
								<p className="mt-0.5 truncate text-tiny text-ink-faint">
									started {formatTimeAgo(status.current_run.started_at, now)}
								</p>
							</>
						) : (
							<>
								<p className="mt-1 font-plex text-xl font-semibold text-ink">
									Idle
								</p>
								<p className="mt-0.5 text-tiny text-ink-faint">
									waiting for the next wake
								</p>
							</>
						)}
					</div>
				</div>

				{/* Advanced */}
				<button
					type="button"
					onClick={() => setAdvancedOpen(!advancedOpen)}
					className="mt-4 flex items-center gap-1 text-tiny font-medium text-ink-faint transition-colors hover:text-ink-dull"
				>
					{advancedOpen ? (
						<CaretDown className="size-3" weight="bold" />
					) : (
						<CaretRight className="size-3" weight="bold" />
					)}
					Advanced
				</button>

				{advancedOpen && (
					<div className="mt-3 space-y-3 rounded-xl border border-app-line/60 p-4">
						<div className="flex items-center justify-between">
							<div>
								<p className="text-sm text-ink-dull">Wake interval</p>
								<p className="text-tiny text-ink-faint">
									How often the agent checks in on its work
								</p>
							</div>
							<div className="flex items-center gap-1">
								{INTERVAL_OPTIONS.map(({label, secs}) => (
									<FilterButton
										key={label}
										label={label}
										active={intervalSecs === secs}
										onClick={() => onUpdate({interval_secs: secs})}
									/>
								))}
							</div>
						</div>

						<div className="flex items-center justify-between">
							<div>
								<p className="text-sm text-ink-dull">Active hours</p>
								<p className="text-tiny text-ink-faint">
									No self-directed activity outside this window
								</p>
							</div>
							<div className="flex items-center gap-1">
								{HOUR_OPTIONS.map(({label, value}) => (
									<FilterButton
										key={label}
										label={label}
										active={
											value === null
												? activeHours === null
												: activeHours !== null &&
													activeHours[0] === value[0] &&
													activeHours[1] === value[1]
										}
										onClick={() =>
											onUpdate({active_hours: value ?? []})
										}
									/>
								))}
							</div>
						</div>

						<div className="flex items-center justify-between">
							<div>
								<p className="text-sm text-ink-dull">Tasks per run</p>
								<p className="text-tiny text-ink-faint">
									Upper bound on how much one wake can take on
								</p>
							</div>
							<div className="flex items-center gap-1">
								{MAX_TASK_OPTIONS.map((n) => (
									<FilterButton
										key={n}
										label={String(n)}
										active={maxTasks === n}
										onClick={() => onUpdate({max_tasks_per_run: n})}
									/>
								))}
							</div>
						</div>
					</div>
				)}
			</CardContent>
		</Card>
	);
}
