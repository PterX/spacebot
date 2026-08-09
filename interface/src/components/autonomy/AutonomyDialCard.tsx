import {useEffect, useState} from "react";
import {
	Power,
	Eye,
	Lightbulb,
	Lightning,
	CaretDown,
	CaretRight,
} from "@phosphor-icons/react";
import {Card, CardContent, FilterButton} from "@spacedrive/primitives";
import type {AutonomyLevel, AutonomyStatus} from "./mock";

const LEVELS: {
	key: AutonomyLevel;
	label: string;
	icon: React.ElementType;
	tagline: string;
}[] = [
	{
		key: "off",
		label: "Off",
		icon: Power,
		tagline: "Responds only when you talk to it. No self-directed activity.",
	},
	{
		key: "observe",
		label: "Observe",
		icon: Eye,
		tagline:
			"Wakes up periodically to review what's happening and keep its notes current. Doesn't create or run anything.",
	},
	{
		key: "suggest",
		label: "Suggest",
		icon: Lightbulb,
		tagline:
			"Researches your goals and prepares proposed work for your review. Nothing runs without your approval.",
	},
	{
		key: "act",
		label: "Act",
		icon: Lightning,
		tagline:
			"Executes work you've approved on its own schedule, and keeps proposing new work. You still decide what gets approved.",
	},
];

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

interface AutonomyDialCardProps {
	level: AutonomyLevel;
	onLevelChange: (level: AutonomyLevel) => void;
	status: AutonomyStatus | undefined;
}

export function AutonomyDialCard({
	level,
	onLevelChange,
	status,
}: AutonomyDialCardProps) {
	const [advancedOpen, setAdvancedOpen] = useState(false);
	const [intervalSecs, setIntervalSecs] = useState(1800);
	const [maxTasks, setMaxTasks] = useState(2);
	const [activeHours, setActiveHours] = useState<[number, number] | null>([8, 22]);
	const now = useNow(1000);

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
							How active your agent is when you're not around.
						</p>
					</div>
				</div>

				{/* Level dial */}
				<div className="grid grid-cols-4 gap-2">
					{LEVELS.map(({key, label, icon: Icon}) => {
						const active = key === level;
						return (
							<button
								key={key}
								type="button"
								onClick={() => onLevelChange(key)}
								className={`flex flex-col items-center gap-2 rounded-xl border px-3 py-4 transition-colors ${
									active
										? "border-accent bg-accent/10 text-ink"
										: "border-app-line text-ink-dull hover:border-app-line hover:bg-app-box/50 hover:text-ink"
								}`}
							>
								<Icon
									className={`size-5 ${active ? "text-accent" : ""}`}
									weight={active ? "fill" : "regular"}
								/>
								<span className="text-sm font-medium">{label}</span>
							</button>
						);
					})}
				</div>

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
									Enriched 2 tasks · proposed 1
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
									every{" "}
									{INTERVAL_OPTIONS.find((o) => o.secs === intervalSecs)
										?.label ?? "30m"}
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
									{status.current_run.activity}
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
										onClick={() => setIntervalSecs(secs)}
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
										onClick={() => setActiveHours(value)}
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
										onClick={() => setMaxTasks(n)}
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
