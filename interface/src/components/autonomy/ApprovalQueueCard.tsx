import {useMutation, useQuery, useQueryClient} from "@tanstack/react-query";
import {Target, UserCircle} from "@phosphor-icons/react";
import {Card, CardHeader, CardContent, Button} from "@spacedrive/primitives";
import {api} from "@/api/client";

function formatTimeAgo(iso: string): string {
	const seconds = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}

interface ApprovalQueueCardProps {
	showAgent?: boolean;
	agentId?: string;
}

export function ApprovalQueueCard({showAgent, agentId}: ApprovalQueueCardProps) {
	const queryClient = useQueryClient();

	const {data} = useQuery({
		queryKey: ["autonomy-pending-tasks", agentId ?? "all"],
		queryFn: () => api.listTasks({status: "pending_approval", agent_id: agentId}),
		staleTime: 30_000,
	});

	const {data: goalsData} = useQuery({
		queryKey: ["goals"],
		queryFn: () => api.listGoals(),
		staleTime: 30_000,
	});

	const {data: agentsData} = useQuery({
		queryKey: ["agents"],
		queryFn: api.agents,
		staleTime: 30_000,
		enabled: !!showAgent,
	});
	const agents = agentsData?.agents ?? [];
	const agentName = (id: string) =>
		agents.find((a) => a.id === id)?.display_name ?? id;
	const goalTitle = (goalId: string | undefined) =>
		goalId
			? (goalsData?.goals.find((g) => g.id === goalId)?.title ?? null)
			: null;

	const invalidate = () => {
		queryClient.invalidateQueries({queryKey: ["autonomy-pending-tasks"]});
		queryClient.invalidateQueries({queryKey: ["tasks"]});
	};

	const approveMutation = useMutation({
		mutationFn: (taskNumber: number) => api.approveTask(taskNumber),
		onSuccess: invalidate,
	});

	// Dismiss moves the proposal back to the backlog instead of deleting it —
	// the enrichment survives and the agent can resurface it later.
	const dismissMutation = useMutation({
		mutationFn: (taskNumber: number) =>
			api.updateTask(taskNumber, {status: "backlog"}),
		onSuccess: invalidate,
	});

	// Hide rows with an in-flight approve/dismiss so the action feels instant.
	const pendingResolutions = new Set<number>();
	if (approveMutation.isPending && approveMutation.variables !== undefined) {
		pendingResolutions.add(approveMutation.variables);
	}
	if (dismissMutation.isPending && dismissMutation.variables !== undefined) {
		pendingResolutions.add(dismissMutation.variables);
	}

	const tasks = (data?.tasks ?? []).filter(
		(t) => !pendingResolutions.has(t.task_number),
	);

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
						{tasks.map((task) => {
							const goal = goalTitle(task.goal_id);
							return (
								<div key={task.id} className="py-4 first:pt-0 last:pb-0">
									<div className="flex items-start justify-between gap-4">
										<div className="min-w-0 flex-1">
											<p className="text-sm font-medium text-ink">
												{task.title}
											</p>
											{task.description && (
												<p className="mt-1 line-clamp-2 text-sm text-ink-dull">
													{task.description}
												</p>
											)}
											<div className="mt-2 flex items-center gap-3 text-tiny text-ink-faint">
												{showAgent && (
													<span className="flex items-center gap-1 text-ink-dull">
														<UserCircle className="size-3.5" />
														{agentName(
															task.assigned_agent_id ??
																task.owner_agent_id,
														)}
													</span>
												)}
												<span>
													proposed {formatTimeAgo(task.created_at)}
												</span>
												{goal && (
													<span className="flex items-center gap-1 text-accent/80">
														<Target className="size-3.5" />
														{goal}
													</span>
												)}
											</div>
										</div>
										<div className="flex shrink-0 items-center gap-1.5">
											<Button
												size="xs"
												variant="accent"
												onClick={() =>
													approveMutation.mutate(task.task_number)
												}
											>
												Approve
											</Button>
											<Button
												size="xs"
												variant="subtle"
												onClick={() =>
													dismissMutation.mutate(task.task_number)
												}
											>
												Dismiss
											</Button>
										</div>
									</div>
								</div>
							);
						})}
					</div>
				)}
			</CardContent>
		</Card>
	);
}
