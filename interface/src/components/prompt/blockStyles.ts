import type {
	BlockLayer,
	BlockSource,
	BlockStability,
	PromptBlock,
} from "@/api/client";

/**
 * One colour per composition layer, held constant across every process type
 * so the eye learns the palette once and reads a worker prompt the same way
 * it reads a channel prompt.
 */
export const LAYER_STYLES: Record<
	BlockLayer,
	{label: string; rule: string; swatch: string; text: string; tint: string}
> = {
	identity: {
		label: "Identity",
		rule: "bg-violet-400",
		swatch: "bg-violet-400",
		text: "text-violet-300",
		tint: "bg-violet-500/[0.06]",
	},
	contract: {
		label: "Contract",
		rule: "bg-zinc-500",
		swatch: "bg-zinc-500",
		text: "text-zinc-300",
		tint: "bg-transparent",
	},
	capabilities: {
		label: "Capabilities",
		rule: "bg-blue-400",
		swatch: "bg-blue-400",
		text: "text-blue-300",
		tint: "bg-blue-500/[0.06]",
	},
	knowledge: {
		label: "Knowledge",
		rule: "bg-emerald-400",
		swatch: "bg-emerald-400",
		text: "text-emerald-300",
		tint: "bg-emerald-500/[0.06]",
	},
	working: {
		label: "Working",
		rule: "bg-amber-400",
		swatch: "bg-amber-400",
		text: "text-amber-300",
		tint: "bg-amber-500/[0.06]",
	},
	runtime: {
		label: "Runtime",
		rule: "bg-rose-400",
		swatch: "bg-rose-400",
		text: "text-rose-300",
		tint: "bg-rose-500/[0.06]",
	},
};

export const LAYER_ORDER: BlockLayer[] = [
	"identity",
	"contract",
	"capabilities",
	"knowledge",
	"working",
	"runtime",
];

export const STABILITY_LABEL: Record<BlockStability, string> = {
	static: "static",
	epoch: "epoch",
	volatile: "volatile",
};

export const SOURCE_LABEL: Record<BlockSource, string> = {
	template: "template",
	file: "file",
	store: "store",
	synthesis: "synthesis",
	live_state: "live state",
	config: "config",
};

/**
 * Template prose that is only whitespace exists to keep the block map tiling
 * the prompt exactly. It carries nothing to read, so the block list hides it
 * while the map still counts its bytes.
 */
export function isJoinery(block: PromptBlock, text: string): boolean {
	return text.slice(block.start, block.end).trim().length === 0;
}

export function formatTokens(tokens: number): string {
	if (tokens < 1000) return `${tokens}`;
	return `${(tokens / 1000).toFixed(1)}k`;
}

export function formatCost(usd: number): string {
	if (usd === 0) return "$0";
	if (usd < 0.01) return `$${usd.toFixed(4)}`;
	return `$${usd.toFixed(3)}`;
}

/** Colour a process kind consistently wherever it is labelled. */
export function processKindStyle(kind: string): string {
	switch (kind) {
		case "channel":
			return "bg-accent/15 text-accent";
		case "branch":
			return "bg-violet-500/15 text-violet-300";
		case "worker":
			return "bg-blue-500/15 text-blue-300";
		case "compactor":
			return "bg-cyan-500/15 text-cyan-300";
		case "chronicle":
		case "chronicle_rollup":
			return "bg-teal-500/15 text-teal-300";
		case "cortex":
			return "bg-emerald-500/15 text-emerald-300";
		default:
			return "bg-app-hover text-ink-dull";
	}
}
