import {useMutation, useQuery, useQueryClient} from "@tanstack/react-query";
import {
	AutonomyDialCard,
	WakesCard,
	ApprovalQueueCard,
	RunHistoryCard,
} from "@/components/autonomy";
import {api, type AutonomyStatus, type AutonomyUpdate} from "@/api/client";

interface AgentAutonomyProps {
	agentId: string;
}

export function AgentAutonomy({agentId}: AgentAutonomyProps) {
	const queryClient = useQueryClient();

	const {data: agentsData} = useQuery({
		queryKey: ["agents"],
		queryFn: api.agents,
		staleTime: 30_000,
	});

	const {data: status} = useQuery({
		queryKey: ["autonomy-status", agentId],
		queryFn: () => api.autonomyStatus(agentId),
		staleTime: 30_000,
	});

	const agent = agentsData?.agents.find((a) => a.id === agentId);

	const configMutation = useMutation({
		mutationFn: (update: AutonomyUpdate) =>
			api.updateAgentConfig({agent_id: agentId, autonomy: update}),
		onMutate: (update) => {
			const previous = queryClient.getQueryData<AutonomyStatus>([
				"autonomy-status",
				agentId,
			]);
			if (previous) {
				const {active_hours, ...rest} = update;
				const patched: AutonomyStatus = {...previous, ...rest};
				if (active_hours !== undefined) {
					patched.active_hours =
						active_hours.length === 2
							? [active_hours[0], active_hours[1]]
							: null;
				}
				queryClient.setQueryData(["autonomy-status", agentId], patched);
			}
			return {previous};
		},
		onError: (_error, _update, context) => {
			if (context?.previous) {
				queryClient.setQueryData(["autonomy-status", agentId], context.previous);
			}
		},
		onSettled: () => {
			queryClient.invalidateQueries({queryKey: ["autonomy-status", agentId]});
			queryClient.invalidateQueries({queryKey: ["autonomy-fleet"]});
			queryClient.invalidateQueries({queryKey: ["agent-config", agentId]});
		},
	});

	return (
		<div className="flex h-full flex-col">
			<div className="min-h-0 flex-1 overflow-y-auto">
				<div className="p-4 pb-12">
					<AutonomyDialCard
						status={status}
						onUpdate={(update) => configMutation.mutate(update)}
						agentName={agent?.display_name ?? agentId}
					/>

					<div className="mt-5">
						<ApprovalQueueCard agentId={agentId} />
					</div>

					<div className="mt-5">
						<WakesCard />
					</div>

					<div className="mt-5">
						<RunHistoryCard agentId={agentId} />
					</div>
				</div>
			</div>
		</div>
	);
}
