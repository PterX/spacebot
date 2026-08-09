import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {Clock, Globe, Lightning, Gauge, Plus} from "@phosphor-icons/react";
import {Card, CardHeader, CardContent, Button} from "@spacedrive/primitives";
import {Toggle} from "@/ui/Toggle";
import {mockAutonomyApi, type WakeTriggerKind} from "./mock";

const TRIGGER_CONFIG: Record<
	WakeTriggerKind,
	{icon: React.ElementType; iconClass: string; label: string}
> = {
	schedule: {icon: Clock, iconClass: "text-blue-400", label: "Schedule"},
	webhook: {icon: Globe, iconClass: "text-violet-400", label: "Webhook"},
	event: {icon: Lightning, iconClass: "text-amber-400", label: "Event"},
	condition: {icon: Gauge, iconClass: "text-emerald-400", label: "Condition"},
};

const LEVEL_LABEL: Record<string, string> = {
	observe: "Observe+",
	suggest: "Suggest+",
	act: "Act only",
};

function formatTimeAgo(iso: string): string {
	const seconds = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
	if (seconds < 60) return "just now";
	if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
	if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
	return `${Math.floor(seconds / 86400)}d ago`;
}

export function WakesCard() {
	const [overrides, setOverrides] = useState<Record<string, boolean>>({});

	const {data} = useQuery({
		queryKey: ["autonomy-wakes"],
		queryFn: mockAutonomyApi.wakes,
		staleTime: 30_000,
	});

	const wakes = data?.wakes ?? [];

	return (
		<Card variant="dark">
			<CardHeader className="flex-row items-center justify-between p-4 pb-3">
				<div className="flex items-center gap-2">
					<h2 className="font-plex text-sm font-medium text-ink-dull">Wakes</h2>
					<span className="text-tiny text-ink-faint">
						what stirs your agent, and what it does when stirred
					</span>
				</div>
				<Button size="xs" variant="subtle">
					<Plus className="mr-1 size-3.5" weight="bold" />
					Add wake
				</Button>
			</CardHeader>

			<CardContent className="px-6 pb-4 pt-0">
				<div className="flex flex-col divide-y divide-app-line/40">
					{wakes.map((wake) => {
						const enabled = overrides[wake.id] ?? wake.enabled;
						const {icon: Icon, iconClass, label} = TRIGGER_CONFIG[wake.trigger_kind];
						return (
							<div
								key={wake.id}
								className={`flex items-center gap-4 py-3 first:pt-0 last:pb-0 ${
									enabled ? "" : "opacity-50"
								}`}
							>
								<span
									className="flex w-28 shrink-0 items-center gap-1.5"
									title={label}
								>
									<Icon className={`size-4 shrink-0 ${iconClass}`} />
									<span className="text-tiny text-ink-faint">{label}</span>
								</span>

								<div className="min-w-0 flex-1">
									<div className="flex items-center gap-2">
										<p className="truncate text-sm font-medium text-ink">
											{wake.name}
										</p>
										<span className="shrink-0 rounded-full bg-app-line/50 px-1.5 py-px text-tiny text-ink-faint">
											{wake.trigger_label}
										</span>
										{wake.builtin && (
											<span className="shrink-0 rounded-full bg-app-line/50 px-1.5 py-px text-tiny text-ink-faint">
												built-in
											</span>
										)}
									</div>
									<p className="mt-0.5 truncate text-tiny text-ink-faint">
										{wake.instructions}
									</p>
								</div>

								<span className="w-16 shrink-0 text-right text-tiny text-ink-faint">
									{LEVEL_LABEL[wake.min_level]}
								</span>
								<span className="w-16 shrink-0 text-right text-tiny tabular-nums text-ink-faint">
									{wake.last_fired_at
										? formatTimeAgo(wake.last_fired_at)
										: "never"}
								</span>
								<Toggle
									size="sm"
									checked={enabled}
									onCheckedChange={(v) =>
										setOverrides((o) => ({...o, [wake.id]: v}))
									}
								/>
							</div>
						);
					})}
				</div>
			</CardContent>
		</Card>
	);
}
