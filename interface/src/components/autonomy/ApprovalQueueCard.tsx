import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {ChatCircleDots, Robot, Target, UserCircle} from "@phosphor-icons/react";
import {Card, CardHeader, CardContent, Button} from "@spacedrive/primitives";
import {api} from "@/api/client";
import {mockAutonomyApi} from "./mock";

function formatTimeAgo(iso: string): string {
	const seconds = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}

interface ApprovalQueueCardProps {
	showAgent?: boolean;
	agentIndex?: number;
}

export function ApprovalQueueCard({showAgent, agentIndex}: ApprovalQueueCardProps) {
	const [resolved, setResolved] = useState<Record<string, "approved" | "dismissed">>(
		{},
	);

	const {data} = useQuery({
		queryKey: ["autonomy-pending"],
		queryFn: mockAutonomyApi.pending,
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

	const tasks = (data?.tasks ?? [])
		.filter((t) => !resolved[t.id])
		.filter((t) => agentIndex === undefined || t.agent_index === agentIndex);

	return (
		<Card variant="dark" className="flex h-full flex-col">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<div className="flex items-center gap-2">
					<h2 className="font-plex text-sm font-medium text-ink-dull">
						Waiting on you
					</h2>
					{tasks.length > 0 && (
						<span className="rounded-full bg-accent/15 px-2 py-0.5 text-tiny font-medium tabular-nums text-accent">
							{tasks.length}
						</span>
					)}
				</div>
			</CardHeader>

			<CardContent className="flex-1 px-6 pb-4 pt-0">
				{tasks.length === 0 ? (
					<div className="flex h-full items-center justify-center py-6">
						<p className="text-sm text-ink-faint">
							Nothing waiting for review. New proposals will land here.
						</p>
					</div>
				) : (
					<div className="flex flex-col divide-y divide-app-line/40">
						{tasks.map((task) => (
							<div key={task.id} className="py-4 first:pt-0 last:pb-0">
								<div className="flex items-start justify-between gap-4">
									<div className="min-w-0 flex-1">
										<p className="text-sm font-medium text-ink">
											{task.title}
										</p>
										<p className="mt-1 line-clamp-2 text-sm text-ink-dull">
											{task.finding}
										</p>
										<div className="mt-2 flex items-center gap-3 text-tiny text-ink-faint">
											{showAgent && (
												<span className="flex items-center gap-1 text-ink-dull">
													<UserCircle className="size-3.5" />
													{agentName(task.agent_index)}
												</span>
											)}
											{task.enriched_at && (
												<span>
													researched {formatTimeAgo(task.enriched_at)}
												</span>
											)}
											<span className="flex items-center gap-1">
												<ChatCircleDots className="size-3.5" />
												{task.comment_count}
											</span>
											<span className="flex items-center gap-1">
												<Robot className="size-3.5" />
												{task.worker_count}
											</span>
											{task.goal_title && (
												<span className="flex items-center gap-1 text-accent/80">
													<Target className="size-3.5" />
													{task.goal_title}
												</span>
											)}
										</div>
									</div>
									<div className="flex shrink-0 items-center gap-1.5">
										<Button
											size="xs"
											variant="accent"
											onClick={() =>
												setResolved((r) => ({...r, [task.id]: "approved"}))
											}
										>
											Approve
										</Button>
										<Button
											size="xs"
											variant="subtle"
											onClick={() =>
												setResolved((r) => ({...r, [task.id]: "dismissed"}))
											}
										>
											Dismiss
										</Button>
									</div>
								</div>
							</div>
						))}
					</div>
				)}
			</CardContent>
		</Card>
	);
}
