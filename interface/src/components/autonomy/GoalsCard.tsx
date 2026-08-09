import {useQuery} from "@tanstack/react-query";
import {Card, CardHeader, CardContent} from "@spacedrive/primitives";
import {mockAutonomyApi, type AutonomyGoal} from "./mock";

const PRIORITY_DOT: Record<AutonomyGoal["priority"], string> = {
	critical: "bg-status-error",
	high: "bg-status-warning",
	medium: "bg-blue-400",
	low: "bg-ink-faint",
};

export function GoalsCard() {
	const {data} = useQuery({
		queryKey: ["autonomy-goals"],
		queryFn: mockAutonomyApi.goals,
		staleTime: 30_000,
	});

	const goals = data?.goals ?? [];

	return (
		<Card variant="dark" className="flex h-full flex-col">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<h2 className="font-plex text-sm font-medium text-ink-dull">Goals</h2>
				<span className="text-tiny text-ink-faint">
					what your agent is working toward
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
							const progress =
								goal.tasks_total > 0 ? goal.tasks_done / goal.tasks_total : 0;
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
											{goal.tasks_done}/{goal.tasks_total}
											{goal.due_date ? ` · due ${goal.due_date}` : ""}
										</span>
									</div>
									<div className="mt-2 h-1.5 overflow-hidden rounded-full bg-app-line/50">
										<div
											className="h-full rounded-full bg-accent"
											style={{width: `${Math.round(progress * 100)}%`}}
										/>
									</div>
									<p className="mt-1.5 line-clamp-1 text-tiny text-ink-faint">
										{goal.notes}
									</p>
								</div>
							);
						})}
					</div>
				)}
			</CardContent>
		</Card>
	);
}
