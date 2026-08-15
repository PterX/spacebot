import type {BlockLayer, BlockSource, BlockStability} from "@/api/client";

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
		rule: "bg-block-identity",
		swatch: "bg-block-identity",
		text: "text-block-identity",
		tint: "bg-block-identity/[0.06]",
	},
	contract: {
		label: "Contract",
		rule: "bg-block-contract",
		swatch: "bg-block-contract",
		text: "text-block-contract",
		tint: "bg-transparent",
	},
	capabilities: {
		label: "Capabilities",
		rule: "bg-block-capabilities",
		swatch: "bg-block-capabilities",
		text: "text-block-capabilities",
		tint: "bg-block-capabilities/[0.06]",
	},
	knowledge: {
		label: "Knowledge",
		rule: "bg-block-knowledge",
		swatch: "bg-block-knowledge",
		text: "text-block-knowledge",
		tint: "bg-block-knowledge/[0.06]",
	},
	working: {
		label: "Working",
		rule: "bg-block-working",
		swatch: "bg-block-working",
		text: "text-block-working",
		tint: "bg-block-working/[0.06]",
	},
	runtime: {
		label: "Runtime",
		rule: "bg-block-runtime",
		swatch: "bg-block-runtime",
		text: "text-block-runtime",
		tint: "bg-block-runtime/[0.06]",
	},
};

/**
 * Look up a layer's styling, tolerating a layer this build does not know.
 *
 * `block.layer` comes from a stored record, so the TypeScript union does not
 * constrain it — a layer added on the Rust side would otherwise dereference
 * `undefined` and unmount the dialog.
 */
export function layerStyle(layer: BlockLayer): (typeof LAYER_STYLES)[BlockLayer] {
	return LAYER_STYLES[layer] ?? LAYER_STYLES.runtime;
}

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
 * Slice a prompt by the byte offsets a block carries.
 *
 * Block ranges are byte offsets into the UTF-8 prompt, while a JavaScript
 * string indexes UTF-16 code units. The two agree only until the first
 * non-ASCII character, after which slicing the string directly drifts further
 * wrong with every multi-byte character above it — so the bytes are sliced as
 * bytes and decoded back.
 */
export function byteSlicer(text: string): (start: number, end: number) => string {
	const bytes = new TextEncoder().encode(text);
	const decoder = new TextDecoder();
	return (start, end) => decoder.decode(bytes.subarray(start, end));
}

export function byteLength(text: string): number {
	return new TextEncoder().encode(text).length;
}

/**
 * Template prose that is only whitespace exists to keep the block map tiling
 * the prompt exactly. It carries nothing to read, so the block list hides it
 * while the map still counts its bytes.
 */
export function isJoinery(text: string): boolean {
	return text.trim().length === 0;
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
			return "bg-block-identity/15 text-block-identity";
		case "worker":
			return "bg-block-capabilities/15 text-block-capabilities";
		case "compactor":
			return "bg-block-tools/15 text-block-tools";
		case "chronicle":
		case "chronicle_rollup":
			return "bg-block-knowledge/15 text-block-knowledge";
		case "cortex":
			return "bg-block-working/15 text-block-working";
		default:
			return "bg-app-hover text-ink-dull";
	}
}
