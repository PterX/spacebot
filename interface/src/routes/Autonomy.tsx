import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {
	AutonomyDialCard,
	WakesCard,
	ApprovalQueueCard,
	GoalsCard,
	RunHistoryCard,
} from "@/components/autonomy";
import {mockAutonomyApi, type AutonomyLevel} from "@/components/autonomy/mock";

export function Autonomy() {
	const {data: status} = useQuery({
		queryKey: ["autonomy-status"],
		queryFn: mockAutonomyApi.status,
		staleTime: 30_000,
	});

	const [levelOverride, setLevelOverride] = useState<AutonomyLevel | null>(null);
	const level = levelOverride ?? status?.level ?? "suggest";

	return (
		<div className="flex h-full flex-col">
			<div className="min-h-0 flex-1 overflow-y-auto">
				<div className="py-3 pr-3 pb-12">
					<AutonomyDialCard
						level={level}
						onLevelChange={setLevelOverride}
						status={status}
					/>

					<div className="mt-5 grid grid-cols-3 gap-5">
						<div className="col-span-2">
							<ApprovalQueueCard />
						</div>
						<GoalsCard />
					</div>

					<div className="mt-5">
						<WakesCard />
					</div>

					<div className="mt-5">
						<RunHistoryCard />
					</div>
				</div>
			</div>
		</div>
	);
}
