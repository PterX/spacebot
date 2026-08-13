import {BookOpenText, CalendarDots} from "@phosphor-icons/react";
import {Card, CardContent, CardHeader} from "@spacedrive/primitives";
import {useQuery} from "@tanstack/react-query";
import {Link} from "@tanstack/react-router";
import {api} from "@/api/client";

export function ChronicleCard() {
	const {data, isLoading, isError} = useQuery({
		queryKey: ["dashboard-chronicle"],
		queryFn: () => api.chronicleHistory(20),
		staleTime: 60_000,
	});
	const {data: agentsData} = useQuery({
		queryKey: ["agents"],
		queryFn: () => api.agents(),
		staleTime: 60_000,
	});

	const agentNames = new Map(
		(agentsData?.agents ?? []).map((agent) => [
			agent.id,
			agent.display_name ?? agent.id,
		]),
	);
	const briefs = data?.daily_briefs ?? [];
	const checkpoints = data?.checkpoints ?? [];

	return (
		<Card variant="dark" className="flex min-h-0 flex-col">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<div>
					<h2 className="font-plex text-sm font-medium text-ink-dull">
						Chronicle
					</h2>
					<p className="mt-0.5 text-tiny text-ink-faint">Across all agents</p>
				</div>
				<BookOpenText className="h-4 w-4 text-violet-400" />
			</CardHeader>

			<CardContent className="px-4 pb-4 pt-0">
				{isLoading ? (
					<div className="py-8 text-center text-sm text-ink-faint">Loading...</div>
				) : isError ? (
					<div className="py-8 text-center text-sm text-status-error">
						Chronicle history could not be loaded.
					</div>
				) : (
					<>
						<div className="mb-4 rounded-xl border border-violet-400/15 bg-violet-400/[0.04] p-3">
							<div className="mb-2 flex items-center gap-2">
								<CalendarDots className="h-4 w-4 text-violet-400" />
								<span className="text-tiny font-medium uppercase tracking-wide text-ink-faint">
									Daily brief
								</span>
							</div>
							{briefs.length === 0 ? (
								<p className="text-sm text-ink-faint">No daily briefs yet.</p>
							) : (
								<div className="flex flex-col divide-y divide-app-line/40">
									{briefs.map((brief) => (
										<div key={brief.agent_id} className="py-2 first:pt-0 last:pb-0">
											<div className="mb-1 flex items-center justify-between gap-3">
												<span className="text-xs font-medium text-ink-dull">
													{agentNames.get(brief.agent_id) ?? brief.agent_id}
												</span>
												<span className="shrink-0 text-tiny tabular-nums text-ink-faint">
													{formatDay(brief.day)}
												</span>
											</div>
											<p className="line-clamp-3 whitespace-pre-wrap text-sm leading-5 text-ink-dull">
												{brief.summary}
											</p>
										</div>
									))}
								</div>
							)}
						</div>

						{checkpoints.length === 0 ? (
							<div className="py-5 text-center text-sm text-ink-faint">
								No chronicle checkpoints yet.
							</div>
						) : (
							<div className="flex flex-col divide-y divide-app-line/40">
								{checkpoints.map((checkpoint) => (
									<Link
										key={`${checkpoint.agent_id}-${checkpoint.id}`}
										to="/agents/$agentId/channels/$channelId"
										params={{
											agentId: checkpoint.agent_id,
											channelId: checkpoint.channel_id,
										}}
										className="group block py-3 first:pt-0 last:pb-0"
									>
										<div className="mb-1 flex items-center justify-between gap-3">
											<span className="text-tiny text-violet-300">
												{agentNames.get(checkpoint.agent_id) ?? checkpoint.agent_id}
											</span>
											<span className="shrink-0 text-tiny tabular-nums text-ink-faint">
												{formatTimeAgo(checkpoint.created_at)}
											</span>
										</div>
										<h3 className="truncate text-sm font-medium text-ink-dull transition-colors group-hover:text-ink">
											{checkpoint.title}
										</h3>
										<p className="mt-1 line-clamp-2 text-xs leading-5 text-ink-faint">
											{checkpoint.summary}
										</p>
									</Link>
								))}
							</div>
						)}
					</>
				)}
			</CardContent>
		</Card>
	);
}

function formatDay(day: string): string {
	return new Date(`${day}T12:00:00`).toLocaleDateString(undefined, {
		month: "short",
		day: "numeric",
	});
}

function formatTimeAgo(timestamp: string): string {
	const seconds = Math.floor((Date.now() - new Date(timestamp).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}
