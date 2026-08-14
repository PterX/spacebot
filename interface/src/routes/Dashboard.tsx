import { ActionItemsCard } from "@/components/dashboard/ActionItemsCard";
import { TokenUsageCard } from "@/components/dashboard/TokenUsageCard";
import { ActivityCard } from "@/components/dashboard/ActivityCard";
import { ChronicleCard } from "@/components/dashboard/ChronicleCard";
import { RecentActivityCard } from "@/components/dashboard/RecentActivityCard";


export function Dashboard() {
	return (
		<div className="flex h-full flex-col">
			<div className="min-h-0 flex-1 overflow-y-auto">
				<div className="py-3 pr-3 pb-12">
					<div className="grid gap-5 lg:h-[340px] lg:grid-cols-2">
						<ActionItemsCard />
						<TokenUsageCard />
					</div>

					<div className="mt-5">
						<ActivityCard />
					</div>

					<div className="mt-5 grid items-start gap-5 xl:grid-cols-2">
						<RecentActivityCard />
						<ChronicleCard />
					</div>
				</div>
			</div>
		</div>
	);
}
