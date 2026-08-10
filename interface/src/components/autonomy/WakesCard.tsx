import {useMutation, useQuery, useQueryClient} from "@tanstack/react-query";
import {Clock, Globe, Lightning} from "@phosphor-icons/react";
import {Card, CardHeader, CardContent} from "@spacedrive/primitives";
import {Toggle} from "@/ui/Toggle";
import {api, type WakeTriggerKind, type WakesResponse} from "@/api/client";

const TRIGGER_CONFIG: Record<
	WakeTriggerKind,
	{icon: React.ElementType; iconClass: string; label: string}
> = {
	schedule: {icon: Clock, iconClass: "text-blue-400", label: "Schedule"},
	webhook: {icon: Globe, iconClass: "text-violet-400", label: "Webhook"},
	event: {icon: Lightning, iconClass: "text-amber-400", label: "Event"},
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

interface WakesCardProps {
	agentId: string;
}

export function WakesCard({agentId}: WakesCardProps) {
	const queryClient = useQueryClient();

	const {data} = useQuery({
		queryKey: ["wakes", agentId],
		queryFn: () => api.listWakes(agentId),
		staleTime: 30_000,
	});

	const toggleMutation = useMutation({
		mutationFn: ({wakeId, enabled}: {wakeId: string; enabled: boolean}) =>
			api.updateWake(agentId, wakeId, {enabled}),
		onMutate: ({wakeId, enabled}) => {
			const previous = queryClient.getQueryData<WakesResponse>(["wakes", agentId]);
			if (previous) {
				queryClient.setQueryData<WakesResponse>(["wakes", agentId], {
					wakes: previous.wakes.map((wake) =>
						wake.id === wakeId ? {...wake, enabled} : wake,
					),
				});
			}
			return {previous};
		},
		onError: (_error, _vars, context) => {
			if (context?.previous) {
				queryClient.setQueryData(["wakes", agentId], context.previous);
			}
		},
		onSettled: () => {
			queryClient.invalidateQueries({queryKey: ["wakes", agentId]});
		},
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
			</CardHeader>

			<CardContent className="px-6 pb-4 pt-0">
				<div className="flex flex-col divide-y divide-app-line/40">
					{wakes.map((wake) => {
						const {icon: Icon, iconClass, label} = TRIGGER_CONFIG[wake.trigger_kind];
						return (
							<div
								key={wake.id}
								className={`flex items-center gap-4 py-3 first:pt-0 last:pb-0 ${
									wake.enabled ? "" : "opacity-50"
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
										{wake.webhook_url && (
											<button
												type="button"
												className="shrink-0 cursor-pointer rounded-full bg-app-line/50 px-1.5 py-px font-mono text-tiny text-ink-faint hover:text-ink"
												title="Copy webhook URL"
												onClick={() =>
													navigator.clipboard.writeText(wake.webhook_url!)
												}
											>
												{wake.webhook_url}
											</button>
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
								<span
									title={
										wake.virtual ? "Governed by the autonomy dial" : undefined
									}
								>
									<Toggle
										size="sm"
										checked={wake.enabled}
										disabled={wake.virtual}
										onCheckedChange={(enabled) =>
											toggleMutation.mutate({wakeId: wake.id, enabled})
										}
									/>
								</span>
							</div>
						);
					})}
				</div>
			</CardContent>
		</Card>
	);
}
