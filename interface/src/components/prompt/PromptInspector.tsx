import {
	useCallback,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import {useQuery} from "@tanstack/react-query";
import {cx} from "class-variance-authority";
import {Check, Copy, X} from "@phosphor-icons/react";
import {
	api,
	type PromptBlock,
	type PromptRecord,
	type PromptRequestSummary,
} from "@/api/client";
import {DialogContent, DialogRoot} from "@spacedrive/primitives";
import {
	LAYER_ORDER,
	LAYER_STYLES,
	SOURCE_LABEL,
	STABILITY_LABEL,
	byteLength,
	byteSlicer,
	formatCost,
	formatTokens,
	isJoinery,
	processKindStyle,
} from "./blockStyles";

/** What set of requests the inspector opens over. */
export type PromptInspectorScope =
	| {kind: "channel"; channelId: string}
	| {kind: "process"; processId: string}
	| {kind: "message"; messageId: string};

interface PromptInspectorProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	agentId?: string;
	scope: PromptInspectorScope;
	/** Preselect a request instead of opening on the most recent one. */
	requestId?: string;
}

export function PromptInspector({
	open,
	onOpenChange,
	agentId,
	scope,
	requestId,
}: PromptInspectorProps) {
	const [selected, setSelected] = useState<string | null>(requestId ?? null);

	const listParams = useMemo(() => {
		switch (scope.kind) {
			case "channel":
				return {agentId, channelId: scope.channelId, limit: 200};
			case "process":
				return {agentId, processId: scope.processId, limit: 200};
			case "message":
				return {agentId, messageId: scope.messageId};
		}
	}, [agentId, scope]);

	const {data: list, isLoading: listLoading} = useQuery({
		queryKey: ["promptRequests", listParams],
		queryFn: () => api.listPromptRequests(listParams),
		enabled: open,
		staleTime: 0,
	});

	// Open on the newest request in scope until the reader picks another.
	const requests = list?.requests ?? [];
	const activeId = selected ?? requests[0]?.request_id ?? null;

	useEffect(() => {
		if (open) setSelected(requestId ?? null);
	}, [open, requestId]);

	const {data: record, isLoading: recordLoading} = useQuery({
		queryKey: ["promptRequest", activeId, agentId],
		queryFn: () => api.getPromptRequest(activeId!, agentId),
		enabled: open && activeId != null,
		staleTime: Infinity,
	});

	return (
		<DialogRoot open={open} onOpenChange={onOpenChange}>
			<DialogContent className="!flex h-[88vh] w-[92vw] !max-w-[1500px] !flex-col !gap-0 overflow-hidden !p-0">
				<InspectorHeader
					record={record}
					captureEnabled={list?.capture_enabled ?? false}
					onClose={() => onOpenChange(false)}
				/>

				<div className="flex min-h-0 flex-1">
					<RequestIndex
						requests={requests}
						loading={listLoading}
						activeId={activeId}
						captureEnabled={list?.capture_enabled ?? false}
						onSelect={setSelected}
					/>

					{recordLoading || !record ? (
						<div className="flex flex-1 items-center justify-center">
							<span className="text-sm text-ink-faint">
								{activeId ? "Loading request…" : "No request selected"}
							</span>
						</div>
					) : (
						<RecordView record={record} />
					)}
				</div>
			</DialogContent>
		</DialogRoot>
	);
}

function InspectorHeader({
	record,
	captureEnabled,
	onClose,
}: {
	record?: PromptRecord;
	captureEnabled: boolean;
	onClose: () => void;
}) {
	return (
		<div className="flex flex-shrink-0 items-start gap-4 border-b border-app-line/50 px-5 py-3.5">
			<div className="min-w-0 flex-1">
				<div className="flex flex-wrap items-center gap-2">
					{record ? (
						<>
							<span
								className={cx(
									"rounded-md px-2 py-0.5 text-[10px] font-medium uppercase tracking-[0.12em]",
									processKindStyle(record.process.kind),
								)}
							>
								{record.process.kind}
							</span>
							{record.process.process_type && (
								<span className="text-tiny text-ink-faint">
									{record.process.process_type}
								</span>
							)}
							<span className="text-ink-faint/40">·</span>
							<span className="text-sm font-medium text-ink">
								{record.model.name}
							</span>
						</>
					) : (
						<span className="text-sm font-medium text-ink">
							Prompt Inspector
						</span>
					)}
				</div>

				{record && (
					<div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-tiny text-ink-faint">
						<span>
							triggered by{" "}
							<span className="text-ink-dull">{record.trigger.kind}</span>
						</span>
						{record.trigger.parent && (
							<span className="truncate">← {record.trigger.parent}</span>
						)}
						{record.trigger.message_id != null && (
							<span>msg #{record.trigger.message_id}</span>
						)}
						<span className="text-ink-faint/40">·</span>
						<span>{new Date(record.started_at).toLocaleString()}</span>
						{record.duration_ms > 0 && (
							<span>{(record.duration_ms / 1000).toFixed(1)}s</span>
						)}
					</div>
				)}
			</div>

			{record && (
				<div className="flex flex-shrink-0 items-center gap-4">
					<Usage record={record} />
					<CopyReference record={record} />
				</div>
			)}

			{!captureEnabled && (
				<span className="rounded-md bg-block-working/10 px-2 py-1 text-tiny text-block-working">
					Capture off
				</span>
			)}

			<button
				type="button"
				onClick={onClose}
				aria-label="Close prompt inspector"
				className="rounded-md p-1.5 text-ink-faint hover:bg-app-hover hover:text-ink"
			>
				<X className="size-4" />
			</button>
		</div>
	);
}

function Usage({record}: {record: PromptRecord}) {
	const {usage} = record;
	// A streamed request records no usage — the response is assembled by the
	// caller, so showing zeros would read as "free" rather than "not measured".
	const measured =
		usage.input_tokens > 0 || usage.output_tokens > 0 || record.duration_ms > 0;
	if (!measured) return null;

	return (
		<div className="flex items-center gap-3 text-tiny">
			<Metric label="in" value={formatTokens(usage.input_tokens)} />
			<Metric label="out" value={formatTokens(usage.output_tokens)} />
			{usage.cached_read_tokens > 0 && (
				<Metric
					label="cached"
					value={formatTokens(usage.cached_read_tokens)}
					accent
				/>
			)}
			{usage.cost_usd > 0 && (
				<span className="font-medium text-ink-dull">
					{formatCost(usage.cost_usd)}
				</span>
			)}
		</div>
	);
}

function Metric({
	label,
	value,
	accent,
}: {
	label: string;
	value: string;
	accent?: boolean;
}) {
	return (
		<span className={accent ? "text-block-knowledge" : "text-ink-dull"}>
			<span className="text-ink-faint">{label} </span>
			{value}
		</span>
	);
}

/**
 * Copies a handle that resolves from a terminal or an agent session without
 * anyone having to know the on-disk layout.
 */
function CopyReference({record: {request_id, agent_id}}: {record: PromptRecord}) {
	const [copied, setCopied] = useState(false);

	const copy = useCallback(() => {
		const short = request_id.slice(0, 8);
		const reference = [
			`spacebot prompt show ${short}`,
			`~/.spacebot/agents/${agent_id}/prompts/*/${request_id}.json`,
		].join("\n");
		navigator.clipboard.writeText(reference).then(() => {
			setCopied(true);
			setTimeout(() => setCopied(false), 1600);
		});
	}, [request_id, agent_id]);

	return (
		<button
			type="button"
			onClick={copy}
			title="Copy a reference to this request"
			className="flex items-center gap-1.5 rounded-md border border-app-line/60 px-2 py-1 font-mono text-tiny text-ink-faint transition-colors hover:bg-app-hover hover:text-ink"
		>
			{copied ? (
				<Check className="size-3 text-status-success" />
			) : (
				<Copy className="size-3" />
			)}
			{request_id.slice(0, 8)}
		</button>
	);
}

function RequestIndex({
	requests,
	loading,
	activeId,
	captureEnabled,
	onSelect,
}: {
	requests: PromptRequestSummary[];
	loading: boolean;
	activeId: string | null;
	captureEnabled: boolean;
	onSelect: (id: string) => void;
}) {
	return (
		<div className="flex w-60 flex-shrink-0 flex-col border-r border-app-line/50 bg-app-dark-box/30">
			<div className="flex items-baseline justify-between px-3 py-2">
				<span className="text-tiny font-medium uppercase tracking-[0.12em] text-ink-faint">
					Requests
				</span>
				<span className="text-tiny text-ink-faint/60">{requests.length}</span>
			</div>

			<div className="flex-1 overflow-y-auto">
				{loading && (
					<p className="px-3 py-2 text-tiny text-ink-faint">Loading…</p>
				)}
				{!loading && requests.length === 0 && (
					<p className="px-3 py-2 text-tiny leading-relaxed text-ink-faint">
						{captureEnabled
							? "Nothing recorded here yet."
							: "Prompt capture is off. Turn it on in Settings to record every request."}
					</p>
				)}
				{requests.map((request) => (
					<RequestRow
						key={request.request_id}
						request={request}
						selected={request.request_id === activeId}
						onClick={() => onSelect(request.request_id)}
					/>
				))}
			</div>
		</div>
	);
}

function RequestRow({
	request,
	selected,
	onClick,
}: {
	request: PromptRequestSummary;
	selected: boolean;
	onClick: () => void;
}) {
	const time = new Date(request.started_at);
	return (
		<button
			type="button"
			onClick={onClick}
			className={cx(
				"w-full border-l-2 px-3 py-2 text-left transition-colors",
				selected
					? "border-l-accent bg-accent/10"
					: "border-l-transparent hover:bg-app-hover",
			)}
		>
			<div className="flex items-center gap-1.5">
				<span
					className={cx(
						"rounded px-1.5 py-px text-[9px] font-medium uppercase tracking-[0.1em]",
						processKindStyle(request.process_kind),
					)}
				>
					{request.process_kind}
				</span>
				<span className="ml-auto text-tiny text-ink-faint">
					{time.toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})}
				</span>
			</div>
			<div className="mt-1 flex gap-2 text-tiny text-ink-faint/70">
				<span>{formatTokens(Math.round(request.system_chars / 4))} sys</span>
				<span>{request.history_length} msg</span>
				{request.tool_count > 0 && <span>{request.tool_count} tools</span>}
			</div>
			{request.status === "error" && (
				<span className="mt-0.5 block text-tiny text-status-error">failed</span>
			)}
		</button>
	);
}

/** A row in the reading column: a prompt block, or a section divider. */
type Row =
	| {kind: "block"; block: PromptBlock; text: string; share: number}
	| {kind: "divider"; id: string; label: string; detail?: string};

function RecordView({record}: {record: PromptRecord}) {
	const scrollRef = useRef<HTMLDivElement>(null);
	const contentRef = useRef<HTMLDivElement>(null);
	const rowRefs = useRef<Record<string, HTMLDivElement | null>>({});

	const {text, blocks} = record.system;

	// Block ranges are byte offsets, so the prompt is sliced as bytes once and
	// the results reused — see `byteSlicer`.
	const {visibleBlocks, blockText, totalBytes} = useMemo(() => {
		const slice = byteSlicer(text);
		const sliced = blocks.map((block) => slice(block.start, block.end));
		return {
			visibleBlocks: blocks.filter((_, index) => !isJoinery(sliced[index]!)),
			blockText: new Map(
				blocks.map((block, index) => [
					`${block.id}-${block.start}`,
					sliced[index]!,
				]),
			),
			totalBytes: byteLength(text) || 1,
		};
	}, [blocks, text]);

	const rows = useMemo<Row[]>(() => {
		const result: Row[] = visibleBlocks.map((block) => ({
			kind: "block" as const,
			block,
			text: blockText.get(`${block.id}-${block.start}`) ?? "",
			share: (block.end - block.start) / totalBytes,
		}));

		// Processes that assemble their prompt outside the template engine
		// carry no block map. The prompt is still the point, so it renders
		// whole rather than not at all.
		if (visibleBlocks.length === 0 && text.length > 0) {
			result.push({
				kind: "divider",
				id: "system",
				label: "System prompt",
				detail: `${text.length.toLocaleString()} chars · no block map`,
			});
		}

		if (record.tools.length > 0) {
			result.push({
				kind: "divider",
				id: "tools",
				label: "Tool definitions",
				detail: `${record.tools.length} tools · ~${formatTokens(
					Math.round(
						record.tools.reduce((sum, tool) => sum + tool.chars, 0) / 4,
					),
				)} tok`,
			});
		}

		result.push({
			kind: "divider",
			id: "messages",
			label: "Message history",
			detail: `${record.history_length} messages`,
		});

		if (record.response.text || record.response.tool_calls.length > 0) {
			result.push({kind: "divider", id: "response", label: "Response"});
		}

		return result;
	}, [visibleBlocks, text, totalBytes, record]);

	const scrollTo = useCallback((id: string) => {
		rowRefs.current[id]?.scrollIntoView({behavior: "smooth", block: "start"});
	}, []);

	const rowKeys = useMemo(
		() =>
			rows.map((row) =>
				row.kind === "block" ? `${row.block.id}-${row.block.start}` : row.id,
			),
		[rows],
	);

	const segments = useDocumentMap(scrollRef, contentRef, rowKeys, rowRefs);

	return (
		<div className="flex min-w-0 flex-1">
			<Minimap
				rows={rows}
				segments={segments}
				scrollRef={scrollRef}
				onJump={scrollTo}
			/>

			<div ref={scrollRef} className="min-w-0 flex-1 overflow-y-auto">
				<div ref={contentRef}>
				{rows.map((row) =>
					row.kind === "block" ? (
						<BlockSection
							key={`${row.block.id}-${row.block.start}`}
							row={row}
							registerRef={(node) => {
								rowRefs.current[`${row.block.id}-${row.block.start}`] = node;
							}}
						/>
					) : (
						<SectionDivider
							key={row.id}
							row={row}
							registerRef={(node) => {
								rowRefs.current[row.id] = node;
							}}
						>
							{row.id === "system" && (
								<pre className="whitespace-pre-wrap break-words px-4 py-3 font-mono text-[11px] leading-[1.55] text-ink-dull">
									{text}
								</pre>
							)}
							{row.id === "tools" && <ToolList record={record} />}
							{row.id === "messages" && <MessageList record={record} />}
							{row.id === "response" && <ResponseView record={record} />}
						</SectionDivider>
					),
				)}
				</div>
			</div>
		</div>
	);
}

/**
 * Measure where each section actually sits in the laid-out document.
 *
 * A block's share of the prompt's bytes is not its share of the rendered
 * height — a 26-character block still costs a header row and padding — so a
 * map drawn from byte shares drifts further from the scrollbar the further
 * down it goes. Measuring the real layout is what keeps the two in step, and
 * it lets the map cover the tool and message sections the prompt blocks do
 * not describe.
 */
function useDocumentMap(
	scrollRef: React.RefObject<HTMLDivElement | null>,
	contentRef: React.RefObject<HTMLDivElement | null>,
	rowKeys: string[],
	rowRefs: React.RefObject<Record<string, HTMLDivElement | null>>,
): Map<string, {top: number; height: number}> {
	const [segments, setSegments] = useState<
		Map<string, {top: number; height: number}>
	>(new Map());

	useLayoutEffect(() => {
		const scroller = scrollRef.current;
		const content = contentRef.current;
		if (!scroller || !content) return;

		const measure = () => {
			const total = scroller.scrollHeight;
			if (!total) return;
			const origin = scroller.getBoundingClientRect().top - scroller.scrollTop;

			const next = new Map<string, {top: number; height: number}>();
			for (const key of rowKeys) {
				const node = rowRefs.current[key];
				if (!node) continue;
				const rect = node.getBoundingClientRect();
				next.set(key, {
					top: (rect.top - origin) / total,
					height: rect.height / total,
				});
			}
			setSegments(next);
		};

		measure();
		const observer = new ResizeObserver(measure);
		observer.observe(content);
		observer.observe(scroller);
		return () => observer.disconnect();
	}, [scrollRef, contentRef, rowKeys, rowRefs]);

	return segments;
}

/**
 * A scaled-down view of the document, drawn from measured layout rather than
 * byte shares so it stays in step with the scrollbar at every depth.
 */
function Minimap({
	rows,
	segments,
	scrollRef,
	onJump,
}: {
	rows: Row[];
	segments: Map<string, {top: number; height: number}>;
	scrollRef: React.RefObject<HTMLDivElement | null>;
	onJump: (id: string) => void;
}) {
	const railRef = useRef<HTMLDivElement>(null);
	const [viewport, setViewport] = useState<{top: number; height: number} | null>(
		null,
	);

	useEffect(() => {
		const node = scrollRef.current;
		if (!node) return;
		const update = () => {
			const total = node.scrollHeight;
			// Nothing to indicate until the content actually overflows; a
			// fraction of a zero height renders as a stray box.
			if (!total || node.clientHeight >= total) {
				setViewport(null);
				return;
			}
			setViewport({
				top: node.scrollTop / total,
				height: node.clientHeight / total,
			});
		};
		update();
		node.addEventListener("scroll", update, {passive: true});
		const observer = new ResizeObserver(update);
		observer.observe(node);
		return () => {
			node.removeEventListener("scroll", update);
			observer.disconnect();
		};
	}, [scrollRef]);

	/** Drag or click anywhere on the rail to scroll to that depth. */
	const seek = useCallback(
		(clientY: number) => {
			const rail = railRef.current;
			const scroller = scrollRef.current;
			if (!rail || !scroller) return;
			const rect = rail.getBoundingClientRect();
			const fraction = (clientY - rect.top) / (rect.height || 1);
			const target =
				fraction * scroller.scrollHeight - scroller.clientHeight / 2;
			scroller.scrollTo({top: Math.max(0, target)});
		},
		[scrollRef],
	);

	if (segments.size === 0) return null;

	return (
		<div
			ref={railRef}
			className="relative w-11 flex-shrink-0 cursor-pointer border-r border-app-line/40 bg-app-dark-box/50"
			onPointerDown={(event) => {
				event.currentTarget.setPointerCapture(event.pointerId);
				seek(event.clientY);
			}}
			onPointerMove={(event) => {
				if (event.buttons === 1) seek(event.clientY);
			}}
		>
			{rows.map((row) => {
				const key =
					row.kind === "block" ? `${row.block.id}-${row.block.start}` : row.id;
				const segment = segments.get(key);
				if (!segment) return null;
				return (
					<button
						key={key}
						type="button"
						title={
							row.kind === "block"
								? `${row.block.id} — ${row.block.chars.toLocaleString()} chars`
								: row.label
						}
						onClick={(event) => {
							event.stopPropagation();
							onJump(key);
						}}
						style={{
							top: `${segment.top * 100}%`,
							// A hairline off the bottom separates neighbours without
							// moving any bar off the depth it maps to.
							height: `max(2px, calc(${segment.height * 100}% - 1px))`,
						}}
						className={cx(
							"absolute inset-x-2 rounded-[2px] opacity-70 transition-opacity hover:opacity-100",
							row.kind === "block"
								? LAYER_STYLES[row.block.layer].swatch
								: SECTION_SWATCH[row.id] ?? "bg-ink-faint/40",
						)}
					/>
				);
			})}

			{viewport && (
				<div
					className="pointer-events-none absolute inset-x-0 rounded-sm border border-ink/30 bg-ink/10"
					style={{
						top: `${viewport.top * 100}%`,
						height: `${Math.max(viewport.height * 100, 1.5)}%`,
					}}
				/>
			)}
		</div>
	);
}

/** Sections below the assembled prompt get their own marks on the map. */
const SECTION_SWATCH: Record<string, string> = {
	system: "bg-block-contract",
	tools: "bg-block-tools",
	messages: "bg-ink-faint/50",
	response: "bg-accent",
};

function BlockSection({
	row,
	registerRef,
}: {
	row: Extract<Row, {kind: "block"}>;
	registerRef: (node: HTMLDivElement | null) => void;
}) {
	const {block, text, share} = row;
	const style = LAYER_STYLES[block.layer];

	return (
		<div ref={registerRef} className="flex scroll-mt-2">
			<div className={cx("w-0.5 flex-shrink-0", style.rule)} />
			<div className={cx("min-w-0 flex-1 px-4 py-3", style.tint)}>
				<div className="mb-1.5 flex flex-wrap items-baseline gap-x-2 gap-y-1">
					<span className={cx("font-mono text-tiny font-medium", style.text)}>
						{block.id}
					</span>
					<span className="text-tiny text-ink-faint">
						{style.label} · {STABILITY_LABEL[block.stability]} ·{" "}
						{SOURCE_LABEL[block.source]}
					</span>
					<span className="ml-auto whitespace-nowrap text-tiny text-ink-faint">
						{block.chars.toLocaleString()} ch · ~{formatTokens(block.tokens)} tok
						· {(share * 100).toFixed(1)}%
					</span>
				</div>
				<pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-[1.55] text-ink-dull">
					{text}
				</pre>
			</div>
		</div>
	);
}

function SectionDivider({
	row,
	registerRef,
	children,
}: {
	row: Extract<Row, {kind: "divider"}>;
	registerRef: (node: HTMLDivElement | null) => void;
	children?: React.ReactNode;
}) {
	return (
		<div ref={registerRef} className="scroll-mt-2">
			<div className="flex items-center gap-3 border-y border-app-line/60 bg-app-dark-box/60 px-4 py-2">
				<span className="text-tiny font-medium uppercase tracking-[0.14em] text-ink-dull">
					{row.label}
				</span>
				{row.detail && (
					<span className="text-tiny text-ink-faint">{row.detail}</span>
				)}
			</div>
			{children}
		</div>
	);
}

function ToolList({record}: {record: PromptRecord}) {
	return (
		<div className="divide-y divide-app-line/20">
			{record.tools.map((tool) => (
				<div key={tool.name} className="px-4 py-2">
					<div className="flex items-baseline gap-2">
						<span className="font-mono text-tiny font-medium text-block-tools">
							{tool.name}
						</span>
						<span className="ml-auto text-tiny text-ink-faint">
							{tool.chars.toLocaleString()} ch
						</span>
					</div>
					<p className="mt-0.5 line-clamp-2 text-tiny leading-snug text-ink-faint">
						{tool.description}
					</p>
				</div>
			))}
		</div>
	);
}

function MessageList({record}: {record: PromptRecord}) {
	const messages = Array.isArray(record.messages) ? record.messages : [];

	if (messages.length === 0) {
		return <p className="px-4 py-3 text-tiny text-ink-faint">(empty history)</p>;
	}

	return (
		<div className="divide-y divide-app-line/20">
			{messages.map((message, index) => (
				<div key={index} className="px-4 py-2.5">
					<span className="font-mono text-tiny uppercase tracking-[0.1em] text-ink-faint">
						{message.role ?? "unknown"}
					</span>
					<pre className="mt-1 whitespace-pre-wrap break-words font-mono text-[11px] leading-[1.55] text-ink-dull">
						{messageText(message)}
					</pre>
				</div>
			))}
		</div>
	);
}

function ResponseView({record}: {record: PromptRecord}) {
	const {response} = record;
	return (
		<div className="px-4 py-3">
			{response.error && (
				<p className="mb-2 rounded-md border border-status-error/20 bg-status-error/10 px-3 py-2 text-tiny text-status-error">
					{response.error}
				</p>
			)}
			{response.tool_calls.length > 0 && (
				<div className="mb-2 flex flex-wrap gap-1.5">
					{response.tool_calls.map((name, index) => (
						<span
							key={`${name}-${index}`}
							className="rounded bg-block-capabilities/15 px-1.5 py-0.5 font-mono text-tiny text-block-capabilities"
						>
							{name}
						</span>
					))}
				</div>
			)}
			{response.text && (
				<pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-[1.55] text-ink">
					{response.text}
				</pre>
			)}
		</div>
	);
}

/** Flatten a rig message into the text a reader wants to see. */
function messageText(message: {role?: string; content?: unknown}): string {
	const content = message.content;
	if (typeof content === "string") return content;

	const parts: string[] = [];
	const blocks = Array.isArray(content) ? content : content ? [content] : [];

	for (const block of blocks as Array<Record<string, unknown>>) {
		if (typeof block.text === "string") {
			parts.push(block.text);
		} else if (block.type === "toolresult") {
			parts.push(`[tool_result] ${JSON.stringify(block.content)}`);
		} else if (block.function) {
			const fn = block.function as {name?: string; arguments?: unknown};
			parts.push(`[tool_use ${fn.name}] ${JSON.stringify(fn.arguments)}`);
		} else if (Array.isArray(block.reasoning)) {
			parts.push(`[thinking] ${block.reasoning.join("\n")}`);
		}
	}

	return parts.join("\n") || "(empty)";
}

/** Legend of the layer palette, for embedding next to the inspector trigger. */
export function LayerLegend() {
	return (
		<div className="flex flex-wrap gap-x-3 gap-y-1">
			{LAYER_ORDER.map((layer) => (
				<span key={layer} className="flex items-center gap-1.5 text-tiny">
					<span
						className={cx("size-2 rounded-[1px]", LAYER_STYLES[layer].swatch)}
					/>
					<span className="text-ink-faint">{LAYER_STYLES[layer].label}</span>
				</span>
			))}
		</div>
	);
}
