import {useQuery} from "@tanstack/react-query";
import {Card, CardHeader, CardContent} from "@spacedrive/primitives";
import {api, type TaskPriority} from "@/api/client";

const PRIORITY_DOT: Record<TaskPriority, string> = {
	critical: "bg-status-error",
	high: "bg-status-warning",
	medium: "bg-blue-400",
	low: "bg-ink-faint",
};

export function GoalsCard() {
	const {data} = useQuery({
		queryKey: ["goals", "active"],
		queryFn: () => api.listGoals({status: "active"}),
		staleTime: 30_000,
	});

	const goals = data?.goals ?? [];

	return (
		<Card variant="dark" className="flex h-full flex-col">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<h2 className="font-plex text-sm font-medium text-ink-dull">Goals</h2>
				<span className="text-tiny text-ink-faint">
					what your agents are working toward
				</span>
			</CardHeader>

			<CardContent className="flex-1 px-6 pb-4 pt-0">
				{goals.length === 0 ? (
					<div className="flex h-full items-center justify-center py-6">
						<p className="text-sm text-ink-faint">
							No goals yet. Give your agent something to work toward.
						</p>
					</div>
				) : (
					<div className="flex flex-col divide-y divide-app-line/40">
						{goals.map((goal) => {
							const counts = goal.task_counts;
							const total =
								counts.pending_approval +
								counts.backlog +
								counts.ready +
								counts.in_progress +
								counts.done +
								counts.failed;
							const progress = total > 0 ? counts.done / total : 0;
							const notes = goal.notes ?? goal.description ?? "";
							return (
								<div key={goal.id} className="py-3.5 first:pt-0 last:pb-0">
									<div className="flex items-center gap-2.5">
										<span
											className={`size-2 shrink-0 rounded-full ${PRIORITY_DOT[goal.priority]}`}
											title={goal.priority}
										/>
										<p className="min-w-0 flex-1 truncate text-sm font-medium text-ink">
											{goal.title}
										</p>
										<span className="shrink-0 text-tiny tabular-nums text-ink-faint">
											{counts.done}/{total}
											{goal.due_date ? ` · due ${goal.due_date}` : ""}
										</span>
									</div>
									<div className="mt-2 h-1.5 overflow-hidden rounded-full bg-app-line/50">
										<div
											className="h-full rounded-full bg-accent"
											style={{width: `${Math.round(progress * 100)}%`}}
										/>
									</div>
									{notes && (
										<p className="mt-1.5 line-clamp-1 text-tiny text-ink-faint">
											{notes}
										</p>
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
