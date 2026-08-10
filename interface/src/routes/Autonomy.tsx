import {useMutation, useQuery, useQueryClient} from "@tanstack/react-query";
import {
	CeilingCard,
	FleetCard,
	ApprovalQueueCard,
	GoalsCard,
	RunHistoryCard,
} from "@/components/autonomy";
import {api, type AutonomyFleetResponse, type AutonomyLevel} from "@/api/client";

export function Autonomy() {
	const queryClient = useQueryClient();

	const {data: fleetData} = useQuery({
		queryKey: ["autonomy-fleet"],
		queryFn: api.autonomyFleet,
		staleTime: 30_000,
	});

	const ceiling = fleetData?.ceiling ?? "act";

	const ceilingMutation = useMutation({
		mutationFn: (level: AutonomyLevel) => api.updateAutonomyCeiling(level),
		onMutate: (level) => {
			const previous = queryClient.getQueryData<AutonomyFleetResponse>([
				"autonomy-fleet",
			]);
			if (previous) {
				queryClient.setQueryData<AutonomyFleetResponse>(["autonomy-fleet"], {
					...previous,
					ceiling: level,
				});
			}
			return {previous};
		},
		onError: (_error, _level, context) => {
			if (context?.previous) {
				queryClient.setQueryData(["autonomy-fleet"], context.previous);
			}
		},
		onSettled: () => {
			queryClient.invalidateQueries({queryKey: ["autonomy-fleet"]});
			queryClient.invalidateQueries({queryKey: ["autonomy-status"]});
		},
	});

	return (
		<div className="flex h-full flex-col">
			<div className="min-h-0 flex-1 overflow-y-auto">
				<div className="py-3 pr-3 pb-12">
					<CeilingCard
						ceiling={ceiling}
						onCeilingChange={(level) => ceilingMutation.mutate(level)}
					/>

					<div className="mt-5">
						<FleetCard ceiling={ceiling} />
					</div>

					<div className="mt-5 grid grid-cols-3 gap-5">
						<div className="col-span-2">
							<ApprovalQueueCard showAgent />
						</div>
						<GoalsCard />
					</div>

					<div className="mt-5">
						<RunHistoryCard showAgent />
					</div>
				</div>
			</div>
		</div>
	);
}
