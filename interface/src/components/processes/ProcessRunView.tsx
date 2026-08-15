import {useEffect, useMemo, useRef, useState} from "react";
import {useQuery} from "@tanstack/react-query";
import {GitBranch, Stop, Wrench, X} from "@phosphor-icons/react";
import {cx} from "class-variance-authority";
import {motion} from "framer-motion";
import {api, type ProcessKind, type ProcessRun, type TranscriptStep} from "@/api/client";
import {LiveDuration} from "@/components/LiveDuration";
import {Markdown} from "@/components/Markdown";
import {ToolCall, pairTranscriptSteps} from "@/components/ToolCall";
import {formatDuration, formatTimeAgo} from "@/lib/format";
import {ProviderIcon} from "@/lib/providerIcons";
import {PromptInspector} from "@/components/prompt/PromptInspector";

export interface ProcessSelection {
	kind: ProcessKind;
	id: string;
}

/**
 * Process data available from live events and channel timelines. API-fetched
 * process details remain full ProcessRun records.
 */
export interface ProcessRunDisplay {
	kind: ProcessKind;
	id: string;
	input: string;
	status: string;
	started_at: string;
	tool_calls?: number;
	output?: string | null;
	completed_at?: string | null;
	transcript?: TranscriptStep[] | null;
	process_type?: string;
	profile?: string | null;
	channel_name?: string | null;
	model?: string | null;
	max_turns?: number | null;
	directory?: string | null;
	interactive?: boolean;
	project_id?: string | null;
}

export function branchRunStatus(conclusion: string | null): string {
	if (conclusion?.startsWith("Branch failed:")) return "failed";
	if (conclusion?.startsWith("Branch cancelled:") || conclusion?.startsWith("Branch was cancelled:")) return "cancelled";
	return "done";
}

interface ProcessCardProps {
	kind: ProcessKind;
	id: string;
	title: string;
	status: string;
	startedAt: string;
	toolCalls?: number;
	currentTool?: string | null;
	processType?: string;
	selected: boolean;
	onSelect: () => void;
	onCancel?: () => void;
}

function ProcessActivity({kind, active}: {kind: ProcessKind; active: boolean}) {
	if (!active) {
		return (
			<div className={cx(
				"flex size-7 items-center justify-center rounded-lg",
				kind === "branch" ? "bg-violet-500/15 text-violet-400" : "bg-blue-500/15 text-blue-400",
			)}>
				{kind === "branch" ? <GitBranch className="size-4" weight="bold" /> : <Wrench className="size-4" weight="bold" />}
			</div>
		);
	}

	return (
		<div className={cx(
			"grid size-7 grid-cols-2 place-content-center gap-[3px] rounded-lg border",
			kind === "branch" ? "border-violet-400/25 bg-violet-500/10" : "border-blue-400/25 bg-blue-500/10",
		)} aria-label={`${kind} running`}>
			{[0, 1, 2, 3].map((index) => (
				<span
					key={index}
					className={cx(
						"size-[4px] animate-pulse rounded-[1px]",
						kind === "branch" ? "bg-violet-400" : "bg-blue-400",
					)}
					style={{animationDelay: `${index * 120}ms`}}
				/>
			))}
		</div>
	);
}

export function ProcessCard({
	kind,
	title,
	status,
	startedAt,
	toolCalls,
	currentTool,
	processType,
	selected,
	onSelect,
	onCancel,
}: ProcessCardProps) {
	const active = status === "running" || status === "idle" || status === "thinking";
	return (
		<div className="flex gap-3 px-3 py-1.5">
			<span className="w-12 flex-shrink-0 pt-3 text-right text-tiny text-ink-faint">
				{active ? <LiveDuration startMs={new Date(startedAt).getTime()} /> : formatTimeAgo(startedAt)}
			</span>
			<div className="group min-w-0 flex-1">
				<button
					type="button"
					onClick={onSelect}
					className={cx(
						"flex w-full items-start gap-3 rounded-xl border px-3.5 py-3 text-left transition-colors",
						selected
							? kind === "branch"
								? "border-violet-400/40 bg-violet-500/10"
								: "border-blue-400/40 bg-blue-500/10"
							: "border-app-line/50 bg-app-box/25 hover:bg-app-box/45",
					)}
				>
					<ProcessActivity kind={kind} active={active && status !== "idle"} />
					<div className="min-w-0 flex-1">
						<div className="flex items-center gap-2">
							<span className="text-tiny font-medium uppercase tracking-[0.12em] text-ink-faint">{kind}</span>
							{kind === "worker" && processType === "opencode" && <ProviderIcon provider="opencode-zen" size={13} />}
							<span className={cx(
								"ml-auto rounded-full px-2 py-0.5 text-[10px] font-medium uppercase tracking-[0.1em]",
								active ? (kind === "branch" ? "bg-violet-500/15 text-violet-300" : "bg-blue-500/15 text-blue-300") : status === "done" ? "bg-status-success/10 text-status-success" : "bg-app-hover text-ink-dull",
							)}>{status === "running" && kind === "branch" ? "thinking" : status}</span>
						</div>
						<p className="mt-1 line-clamp-2 text-sm font-medium leading-5 text-ink">{title}</p>
						<div className="mt-1.5 flex min-w-0 items-center gap-2 text-tiny text-ink-faint">
							{toolCalls !== undefined && toolCalls > 0 && <span>{toolCalls} tool call{toolCalls === 1 ? "" : "s"}</span>}
							{toolCalls !== undefined && toolCalls > 0 && currentTool && <span>·</span>}
							{currentTool && <span className="truncate">{currentTool}</span>}
						</div>
					</div>
				</button>
				{onCancel && active && (
					<button
						type="button"
						onClick={onCancel}
						className="mt-1 flex items-center gap-1 rounded-md px-2 py-1 text-tiny text-ink-faint opacity-0 transition-opacity hover:bg-status-error/10 hover:text-status-error group-hover:opacity-100 group-focus-within:opacity-100"
					>
						<Stop className="size-3" /> Cancel
					</button>
				)}
			</div>
		</div>
	);
}

function durationBetween(start: string, end: string | null): string | null {
	if (!end) return null;
	const seconds = Math.max(0, Math.floor((new Date(end).getTime() - new Date(start).getTime()) / 1000));
	return formatDuration(seconds);
}

function withoutDuplicateOutput(transcript: TranscriptStep[] | null, output: string | null) {
	if (!transcript || !output) return transcript;
	const last = transcript[transcript.length - 1];
	if (last?.type === "action" && last.content.length === 1 && last.content[0].type === "text" && last.content[0].text.trim() === output.trim()) {
		return transcript.slice(0, -1);
	}
	return transcript;
}

interface ProcessDetailProps {
	agentId: string;
	selection: ProcessSelection;
	fallback: ProcessRunDisplay;
	liveTranscript?: TranscriptStep[];
	onClose: () => void;
}

export function ProcessDetail({agentId, selection, fallback, liveTranscript, onClose}: ProcessDetailProps) {
	const active = fallback.status === "running" || fallback.status === "idle";
	const detailQuery = useQuery({
		queryKey: ["process-detail", agentId, selection.kind, selection.id],
		queryFn: () => api.processDetail(agentId, selection.kind, selection.id),
		refetchInterval: active ? 1500 : false,
	});
	const detail: ProcessRun | ProcessRunDisplay = detailQuery.data ?? fallback;
	const isLive = detail.status === "running" || detail.status === "idle";
	const rawTranscript = isLive && liveTranscript?.length ? liveTranscript : detail.transcript;
	const transcript = useMemo(() => withoutDuplicateOutput(rawTranscript ?? null, detail.output ?? null), [rawTranscript, detail.output]);
	const transcriptRef = useRef<HTMLDivElement>(null);
	const [inspectorOpen, setInspectorOpen] = useState(false);

	useEffect(() => {
		if (isLive && transcriptRef.current) transcriptRef.current.scrollTop = transcriptRef.current.scrollHeight;
	}, [isLive, transcript?.length]);

	const metadata = [
		["Kind", detail.kind],
		["Type", detail.process_type],
		["Profile", detail.profile],
		["Model", detail.model],
		["Max turns", detail.max_turns?.toString()],
		["Mode", detail.interactive === true ? "interactive" : detail.interactive === false && detail.kind === "worker" ? "one-shot" : null],
		["Directory", detail.directory],
		["Project", detail.project_id],
	].filter((entry): entry is [string, string] => Boolean(entry[1]));

	return (
		<div className="flex h-full min-h-0 flex-col bg-app-dark-box/95">
		<div className="border-b border-app-line/50 px-5 py-3">
			<div className="flex items-center gap-3">
				<ProcessActivity kind={detail.kind} active={isLive && detail.status !== "idle"} />
				<div className="min-w-0 flex-1">
					<div className="flex items-center gap-2 text-tiny font-medium uppercase tracking-[0.12em] text-ink-faint">
						<span>{detail.kind}</span><span>·</span><span>{detail.status}</span>
					</div>
					<div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-tiny text-ink-faint">
						{isLive ? <span>Running for <LiveDuration startMs={new Date(detail.started_at).getTime()} /></span> : detail.completed_at && <span>{durationBetween(detail.started_at, detail.completed_at)}</span>}
						{detail.tool_calls !== undefined && <span>{detail.tool_calls} tool call{detail.tool_calls === 1 ? "" : "s"}</span>}
						{detail.channel_name && <span>{detail.channel_name}</span>}
					</div>
				</div>
				<button
					type="button"
					onClick={() => setInspectorOpen(true)}
					className="rounded-md px-2 py-1 text-tiny text-ink-faint transition-colors hover:bg-app-hover hover:text-ink"
					title="Inspect the prompts this process sent"
				>
					Inspect prompt
				</button>
				<button type="button" onClick={onClose} className="rounded-md p-1.5 text-ink-faint hover:bg-app-hover hover:text-ink" aria-label="Close process detail">
					<X className="size-4" />
				</button>
			</div>
		</div>

		<div ref={transcriptRef} className="flex-1 overflow-y-auto">
			{metadata.length > 0 && (
				<section className="border-b border-app-line/40 px-5 py-2.5">
					<h3 className="mb-1.5 text-tiny font-medium text-ink-faint">Configuration</h3>
					<dl className="flex flex-wrap gap-x-4 gap-y-1">
						{metadata.map(([label, value]) => <div key={label} className={cx("flex min-w-0 gap-1 text-tiny", label === "Directory" && "basis-full")}><dt className="text-ink-faint">{label}:</dt><dd className="truncate text-ink-dull" title={value}>{value}</dd></div>)}
					</dl>
				</section>
			)}
			{detail.output && (
				<section className="border-b border-app-line/40 px-5 py-4">
					<h3 className="mb-2 text-tiny font-medium uppercase tracking-wider text-ink-faint">Output</h3>
					<div className="text-xs text-ink"><Markdown>{detail.output}</Markdown></div>
				</section>
			)}
			<section className="px-5 py-4">
				<div className="flex flex-col gap-3">
					<div>
						<p className="mb-1 text-tiny font-medium text-ink-faint">Input</p>
						<div className="text-xs leading-5 text-ink"><Markdown>{detail.input}</Markdown></div>
					</div>
					{transcript?.length ? (
						<>
						{pairTranscriptSteps(transcript).map((item, index) => (
							<motion.div key={item.kind === "tool" ? item.pair.id : `text-${index}`} initial={{opacity: 0, y: 4}} animate={{opacity: 1, y: 0}}>
								{item.kind === "tool" ? <ToolCall pair={item.pair} /> : <div className="text-xs text-ink-dull"><Markdown>{item.text.replace(/ {3,}/g, "  ")}</Markdown></div>}
							</motion.div>
						))}
						</>
					) : detailQuery.isLoading ? <p className="text-xs text-ink-faint">Loading transcript...</p> : <p className="text-xs text-ink-faint">{isLive ? "Waiting for the first tool call..." : "No transcript recorded."}</p>}
				</div>
			</section>
		</div>

		<PromptInspector
			open={inspectorOpen}
			onOpenChange={setInspectorOpen}
			agentId={agentId}
			scope={{kind: "process", processId: selection.id}}
		/>
		</div>
	);
}
