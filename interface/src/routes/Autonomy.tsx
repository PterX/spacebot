import {useState} from "react";
import {
	CeilingCard,
	FleetCard,
	ApprovalQueueCard,
	GoalsCard,
	RunHistoryCard,
} from "@/components/autonomy";
import type {AutonomyLevel} from "@/components/autonomy/mock";

export function Autonomy() {
	const [ceiling, setCeiling] = useState<AutonomyLevel>("act");

	return (
		<div className="flex h-full flex-col">
			<div className="min-h-0 flex-1 overflow-y-auto">
				<div className="py-3 pr-3 pb-12">
					<CeilingCard ceiling={ceiling} onCeilingChange={setCeiling} />

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
