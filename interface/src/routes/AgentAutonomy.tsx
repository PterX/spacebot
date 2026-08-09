import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {
	AutonomyDialCard,
	WakesCard,
	ApprovalQueueCard,
	RunHistoryCard,
} from "@/components/autonomy";
import {
	mockAutonomyApi,
	type AutonomyLevel,
	type AutonomyStatus,
} from "@/components/autonomy/mock";
import {api} from "@/api/client";

interface AgentAutonomyProps {
	agentId: string;
}

export function AgentAutonomy({agentId}: AgentAutonomyProps) {
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
	const agentIndex = Math.max(
		0,
		agents.findIndex((a) => a.id === agentId),
	);
	const agent = agents[agentIndex];
	const state = fleetData?.states[agentIndex % (fleetData.states.length || 1)];

	const [levelOverride, setLevelOverride] = useState<AutonomyLevel | null>(null);
	const level = levelOverride ?? state?.level ?? "suggest";

	const status: AutonomyStatus | undefined = state
		? {
				level: state.level,
				interval_secs: 1800,
				active_hours: [8, 22],
				max_tasks_per_run: 2,
				last_run_at: state.last_run_at,
				next_run_at: state.next_run_at,
				current_run: state.current_run,
			}
		: undefined;

	return (
		<div className="flex h-full flex-col">
			<div className="min-h-0 flex-1 overflow-y-auto">
				<div className="p-4 pb-12">
					<AutonomyDialCard
						level={level}
						onLevelChange={setLevelOverride}
						status={status}
						agentName={agent?.display_name ?? agentId}
					/>

					<div className="mt-5">
						<ApprovalQueueCard agentIndex={agentIndex} />
					</div>

					<div className="mt-5">
						<WakesCard />
					</div>

					<div className="mt-5">
						<RunHistoryCard agentIndex={agentIndex} />
					</div>
				</div>
			</div>
		</div>
	);
}
