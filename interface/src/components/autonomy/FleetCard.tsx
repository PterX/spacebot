import {useQuery} from "@tanstack/react-query";
import {Link} from "@tanstack/react-router";
import {CaretRight, ArrowBendDownRight} from "@phosphor-icons/react";
import {Card, CardHeader, CardContent} from "@spacedrive/primitives";
import {api} from "@/api/client";
import {ProfileAvatar} from "@/components/ProfileAvatar";
import {LEVELS, effectiveLevel} from "./levels";
import {mockAutonomyApi, type AutonomyLevel} from "./mock";

function formatTimeAgo(iso: string): string {
	const seconds = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}

interface FleetCardProps {
	ceiling: AutonomyLevel;
}

export function FleetCard({ceiling}: FleetCardProps) {
	const {data: agentsData} = useQuery({
		queryKey: ["agents"],
		queryFn: api.agents,
		staleTime: 30_000,
	});

	const {data: fleetData} = useQuery({
		queryKey: ["autonomy-fleet"],
		queryFn: mockAutonomyApi.fleet,
		staleTime: 30_000,
	});

	const agents = agentsData?.agents ?? [];
	const states = fleetData?.states ?? [];

	return (
		<Card variant="dark">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<h2 className="font-plex text-sm font-medium text-ink-dull">Agents</h2>
				<span className="text-tiny text-ink-faint">
					each has its own dial — click through to adjust
				</span>
			</CardHeader>

			<CardContent className="px-6 pb-4 pt-0">
				{agents.length === 0 ? (
					<div className="py-6 text-center">
						<p className="text-sm text-ink-faint">No agents yet</p>
					</div>
				) : (
					<div className="flex flex-col divide-y divide-app-line/40">
						{agents.map((agent, i) => {
							const state = states[i % Math.max(states.length, 1)];
							if (!state) return null;
							const name = agent.display_name ?? agent.id;
							const effective = effectiveLevel(ceiling, state.level);
							const capped = effective !== state.level;
							const levelMeta = LEVELS.find((l) => l.key === state.level);
							const effectiveMeta = LEVELS.find((l) => l.key === effective);
							const LevelIcon = levelMeta?.icon ?? CaretRight;

							return (
								<Link
									key={agent.id}
									to={`/agents/$agentId/autonomy`}
									params={{agentId: agent.id}}
									className="group flex items-center gap-4 py-3.5 first:pt-0 last:pb-0"
								>
									<ProfileAvatar
										seed={agent.id}
										name={name}
										size={28}
										className="shrink-0 rounded-full"
										gradientStart={agent.gradient_start}
										gradientEnd={agent.gradient_end}
									/>

									<div className="min-w-0 flex-1">
										<div className="flex items-center gap-2">
											<p className="truncate text-sm font-medium text-ink">
												{name}
											</p>
											<span className="flex shrink-0 items-center gap-1 rounded-full bg-app-line/50 px-2 py-0.5 text-tiny text-ink-dull">
												<LevelIcon className="size-3" />
												{levelMeta?.label}
											</span>
											{capped && (
												<span className="flex shrink-0 items-center gap-1 rounded-full bg-status-warning/15 px-2 py-0.5 text-tiny text-status-warning">
													<ArrowBendDownRight className="size-3" />
													capped to {effectiveMeta?.label}
												</span>
											)}
										</div>
										<p className="mt-0.5 truncate text-tiny text-ink-faint">
											{state.last_run_at
												? `Last run ${formatTimeAgo(state.last_run_at)} · ${state.last_summary}`
												: "No runs yet"}
										</p>
									</div>

									<span className="w-40 shrink-0 text-right text-tiny text-ink-faint">
										{effective === "off" ? (
											"paused"
										) : state.current_run ? (
											<span className="inline-flex items-center gap-1.5 text-status-success">
												<span className="relative flex size-2">
													<span className="absolute inline-flex size-full animate-ping rounded-full bg-status-success opacity-60" />
													<span className="relative inline-flex size-2 rounded-full bg-status-success" />
												</span>
												working now
											</span>
										) : state.next_run_at ? (
											`next run in ${Math.max(
												1,
												Math.round(
													(new Date(state.next_run_at).getTime() -
														Date.now()) /
														60_000,
												),
											)}m`
										) : (
											"idle"
										)}
									</span>

									<span className="w-20 shrink-0 text-right text-tiny tabular-nums text-ink-faint">
										{state.pending_count > 0
											? `${state.pending_count} pending`
											: "—"}
									</span>

									<CaretRight className="size-3.5 shrink-0 text-ink-faint transition-transform group-hover:translate-x-0.5" />
								</Link>
							);
						})}
					</div>
				)}
			</CardContent>
		</Card>
	);
}
