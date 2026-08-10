import {Card, CardContent} from "@spacedrive/primitives";
import {LevelDial} from "./levels";
import type {AutonomyLevel} from "@/api/client";

const CEILING_TAGLINES: Record<AutonomyLevel, string> = {
	off: "Fleet paused. No agent acts on its own, no matter its own setting.",
	observe:
		"Agents may watch and summarize only — anything higher is capped until you raise the ceiling.",
	suggest:
		"Agents may research and propose work for your review. Execution is capped fleet-wide.",
	act: "No cap. Every agent runs at its own setting.",
};

interface CeilingCardProps {
	/** Undefined while fleet data loads; the dial renders inert with no
	 * selection until the real value arrives. */
	ceiling?: AutonomyLevel;
	onCeilingChange: (level: AutonomyLevel) => void;
}

export function CeilingCard({ceiling, onCeilingChange}: CeilingCardProps) {
	return (
		<Card variant="dark">
			<CardContent className="p-6">
				<div className="mb-5">
					<h1 className="font-plex text-lg font-semibold text-ink">Autonomy</h1>
					<p className="mt-0.5 text-sm text-ink-faint">
						How active your agents are allowed to be.
					</p>
				</div>

				<div
					className={
						ceiling === undefined ? "pointer-events-none opacity-50" : undefined
					}
				>
					<LevelDial value={ceiling} onChange={onCeilingChange} />
				</div>

				<p className="mt-3 min-h-[2.5rem] text-sm text-ink-dull">
					{ceiling !== undefined ? CEILING_TAGLINES[ceiling] : ""}
				</p>

				<p className="mt-1 text-tiny text-ink-faint">
					This is a ceiling, not a switch — each agent keeps its own dial below.
					Lower it to pause the fleet; raise it and every agent resumes at its
					own setting.
				</p>
			</CardContent>
		</Card>
	);
}
