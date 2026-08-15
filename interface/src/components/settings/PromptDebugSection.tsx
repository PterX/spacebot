import {useEffect, useState} from "react";
import {useMutation, useQuery, useQueryClient} from "@tanstack/react-query";
import {api} from "@/api/client";
import {Button} from "@spacedrive/primitives";
import {Toggle} from "@/ui/Toggle";
import {LayerLegend} from "@/components/prompt/PromptInspector";

const RETENTION_OPTIONS = [1, 3, 7, 14, 30];

export function PromptDebugSection() {
	const queryClient = useQueryClient();
	const [retentionDays, setRetentionDays] = useState(7);

	const {data: settings, isLoading} = useQuery({
		queryKey: ["prompt-debug-capture"],
		queryFn: () => api.getPromptDebugCapture(),
	});

	useEffect(() => {
		if (settings) setRetentionDays(settings.retention_days);
	}, [settings]);

	const [error, setError] = useState<string | null>(null);

	const mutation = useMutation({
		mutationFn: (next: {enabled: boolean; retentionDays?: number}) =>
			api.setPromptDebugCapture(next.enabled, {
				retentionDays: next.retentionDays,
			}),
		onMutate: () => setError(null),
		onSuccess: () => {
			queryClient.invalidateQueries({queryKey: ["prompt-debug-capture"]});
			queryClient.invalidateQueries({queryKey: ["promptRequests"]});
		},
		// Without this a rejected write leaves the retention buttons showing a
		// value the server never accepted.
		onError: (failure: Error) => {
			setError(failure.message);
			queryClient.invalidateQueries({queryKey: ["prompt-debug-capture"]});
		},
	});

	const enabled = settings?.enabled ?? false;

	return (
		<div className="mx-auto max-w-2xl px-6 py-6">
			<div className="mb-6">
				<h2 className="font-plex text-sm font-semibold text-ink">
					Prompt Capture
				</h2>
				<p className="mt-1 text-sm text-ink-dull">
					Record every request sent to a model — channels, branches, workers,
					compaction, chronicle and cortex runs — so any of them can be opened
					in the prompt inspector.
				</p>
			</div>

			{isLoading ? (
				<div className="flex items-center gap-2 text-ink-dull">
					<div className="h-2 w-2 animate-pulse rounded-full bg-accent" />
					Loading settings...
				</div>
			) : (
				<div className="flex flex-col gap-4">
					<div className="flex items-start gap-4 rounded-lg border border-app-line bg-app-box p-4">
						<div className="flex-1">
							<span className="text-sm font-medium text-ink">
								Capture every request
							</span>
							<p className="mt-0.5 text-sm text-ink-dull">
								Payloads are written to the agent's data directory. They are
								large — a busy channel writes tens of megabytes a day — so
								retention below bounds them.
							</p>
						</div>
						<Toggle
							checked={enabled}
							onCheckedChange={(next) =>
								mutation.mutate({enabled: next, retentionDays})
							}
							disabled={mutation.isPending}
						/>
					</div>

					<div className="rounded-lg border border-app-line bg-app-box p-4">
						<span className="text-sm font-medium text-ink">Keep for</span>
						<div className="mt-2 flex flex-wrap gap-2">
							{RETENTION_OPTIONS.map((days) => (
								<Button
									key={days}
									size="sm"
									variant={retentionDays === days ? "accent" : "gray"}
									// Overlapping writes can land out of order, leaving the
									// selection disagreeing with what was stored last.
									disabled={mutation.isPending}
									onClick={() => {
										setRetentionDays(days);
										mutation.mutate({enabled, retentionDays: days});
									}}
								>
									{days} {days === 1 ? "day" : "days"}
								</Button>
							))}
						</div>
						<p className="mt-2 text-sm text-ink-dull">
							Older records are swept every six hours, payloads and index
							together.
						</p>
					</div>

					{error && (
						<div className="rounded-md border border-status-error/20 bg-status-error/10 px-3 py-2 text-sm text-status-error">
							Failed to update prompt capture: {error}
						</div>
					)}

					<div className="rounded-lg border border-app-line bg-app-box p-4">
						<span className="text-sm font-medium text-ink">Block layers</span>
						<p className="mt-0.5 mb-2 text-sm text-ink-dull">
							Colours the inspector uses for the parts a prompt is assembled
							from.
						</p>
						<LayerLegend />
					</div>
				</div>
			)}
		</div>
	);
}
