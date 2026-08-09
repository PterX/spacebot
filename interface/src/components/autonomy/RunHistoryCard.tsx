import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {
	CaretDown,
	CaretRight,
	MagnifyingGlass,
	PlusCircle,
	Play,
	MoonStars,
	Clock,
	Globe,
	Lightning,
	Gauge,
} from "@phosphor-icons/react";
import {Card, CardHeader, CardContent} from "@spacedrive/primitives";
import {api} from "@/api/client";
import {
	mockAutonomyApi,
	type AutonomyRunAction,
	type WakeTriggerKind,
} from "./mock";

const WAKE_ICON: Record<WakeTriggerKind, React.ElementType> = {
	schedule: Clock,
	webhook: Globe,
	event: Lightning,
	condition: Gauge,
};

const ACTION_CONFIG: Record<
	AutonomyRunAction["kind"],
	{icon: React.ElementType; iconClass: string; label: string}
> = {
	enriched: {
		icon: MagnifyingGlass,
		iconClass: "text-blue-400",
		label: "Researched",
	},
	created: {icon: PlusCircle, iconClass: "text-violet-400", label: "Proposed"},
	executed: {icon: Play, iconClass: "text-status-success", label: "Executed"},
};

function formatTimeAgo(iso: string): string {
	const seconds = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}

function formatDuration(secs: number): string {
	const m = Math.floor(secs / 60);
	const s = secs % 60;
	if (m === 0) return `${s}s`;
	return `${m}m ${s}s`;
}

interface RunHistoryCardProps {
	showAgent?: boolean;
	agentIndex?: number;
}

export function RunHistoryCard({showAgent, agentIndex}: RunHistoryCardProps) {
	const [expanded, setExpanded] = useState<Record<string, boolean>>({});

	const {data} = useQuery({
		queryKey: ["autonomy-runs"],
		queryFn: mockAutonomyApi.runs,
		staleTime: 30_000,
	});

	const {data: agentsData} = useQuery({
		queryKey: ["agents"],
		queryFn: api.agents,
		staleTime: 30_000,
		enabled: !!showAgent,
	});
	const agents = agentsData?.agents ?? [];
	const agentName = (i: number) =>
		agents[i]?.display_name ?? agents[i]?.id ?? `agent ${i + 1}`;

	const runs = (data?.runs ?? []).filter(
		(r) => agentIndex === undefined || r.agent_index === agentIndex,
	);

	return (
		<Card variant="dark">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<h2 className="font-plex text-sm font-medium text-ink-dull">
					Run History
				</h2>
				<span className="text-tiny text-ink-faint">
					everything the agent did on its own
				</span>
			</CardHeader>

			<CardContent className="px-6 pb-4 pt-0">
				{runs.length === 0 ? (
					<div className="py-6 text-center">
						<p className="text-sm text-ink-faint">No runs yet</p>
					</div>
				) : (
					<div className="flex flex-col divide-y divide-app-line/40">
						{runs.map((run) => {
							const isOpen = !!expanded[run.id];
							const idle = run.actions.length === 0;
							return (
								<div key={run.id} className="py-2.5 first:pt-0 last:pb-0">
									<button
										type="button"
										disabled={idle}
										onClick={() =>
											setExpanded((e) => ({...e, [run.id]: !e[run.id]}))
										}
										className={`flex w-full items-center gap-3 text-left ${
											idle ? "cursor-default" : ""
										}`}
									>
										<span className="flex size-4 shrink-0 items-center justify-center text-ink-faint">
											{idle ? (
												<MoonStars className="size-4" />
											) : isOpen ? (
												<CaretDown className="size-3.5" weight="bold" />
											) : (
												<CaretRight className="size-3.5" weight="bold" />
											)}
										</span>
										<span className="flex min-w-0 flex-1 items-center gap-2">
											{showAgent && (
												<span className="shrink-0 rounded-full bg-app-line/40 px-2 py-0.5 text-tiny text-ink-dull">
													{agentName(run.agent_index)}
												</span>
											)}
											<p
												className={`min-w-0 truncate text-sm ${
													idle ? "text-ink-faint" : "text-ink-dull"
												}`}
											>
												{run.summary}
											</p>
											{run.woken_by.map((w, i) => {
												const WakeIcon = WAKE_ICON[w.kind];
												return (
													<span
														key={i}
														className="flex max-w-56 shrink-0 items-center gap-1 rounded-full bg-app-line/40 px-2 py-0.5 text-tiny text-ink-faint"
														title={w.label}
													>
														<WakeIcon className="size-3 shrink-0" />
														<span className="truncate">{w.label}</span>
													</span>
												);
											})}
										</span>
										<span className="shrink-0 text-tiny tabular-nums text-ink-faint">
											{formatDuration(run.duration_secs)}
										</span>
										<span className="w-16 shrink-0 text-right text-tiny tabular-nums text-ink-faint">
											{formatTimeAgo(run.started_at)}
										</span>
									</button>

									{isOpen && !idle && (
										<div className="mt-2 flex flex-col gap-2 pb-1 pl-7">
											{run.actions.map((action, i) => {
												const {
													icon: Icon,
													iconClass,
													label,
												} = ACTION_CONFIG[action.kind];
												return (
													<div key={i} className="flex items-start gap-2.5">
														<Icon
															className={`mt-0.5 size-4 shrink-0 ${iconClass}`}
														/>
														<div className="min-w-0">
															<p className="text-sm text-ink-dull">
																<span className="text-ink-faint">
																	{label}:
																</span>{" "}
																{action.task_title}
															</p>
															<p className="text-tiny text-ink-faint">
																{action.detail}
															</p>
														</div>
													</div>
												);
											})}
										</div>
									)}
								</div>
							);
						})}
					</div>
				)}
			</CardContent>
		</Card>
	);
}
