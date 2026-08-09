import {Power, Eye, Lightbulb, Lightning} from "@phosphor-icons/react";
import type {AutonomyLevel} from "./mock";

export const LEVELS: {
	key: AutonomyLevel;
	label: string;
	icon: React.ElementType;
	tagline: string;
}[] = [
	{
		key: "off",
		label: "Off",
		icon: Power,
		tagline: "Responds only when you talk to it. No self-directed activity.",
	},
	{
		key: "observe",
		label: "Observe",
		icon: Eye,
		tagline:
			"Wakes up periodically to review what's happening and keep its notes current. Doesn't create or run anything.",
	},
	{
		key: "suggest",
		label: "Suggest",
		icon: Lightbulb,
		tagline:
			"Researches your goals and prepares proposed work for your review. Nothing runs without your approval.",
	},
	{
		key: "act",
		label: "Act",
		icon: Lightning,
		tagline:
			"Executes work you've approved on its own schedule, and keeps proposing new work. You still decide what gets approved.",
	},
];

export const LEVEL_ORDER: AutonomyLevel[] = ["off", "observe", "suggest", "act"];

export function levelIndex(level: AutonomyLevel): number {
	return LEVEL_ORDER.indexOf(level);
}

export function effectiveLevel(
	ceiling: AutonomyLevel,
	level: AutonomyLevel,
): AutonomyLevel {
	return LEVEL_ORDER[Math.min(levelIndex(ceiling), levelIndex(level))];
}

interface LevelDialProps {
	value: AutonomyLevel;
	onChange: (level: AutonomyLevel) => void;
}

export function LevelDial({value, onChange}: LevelDialProps) {
	return (
		<div className="grid grid-cols-4 gap-2">
			{LEVELS.map(({key, label, icon: Icon}) => {
				const active = key === value;
				return (
					<button
						key={key}
						type="button"
						onClick={() => onChange(key)}
						className={`flex flex-col items-center gap-2 rounded-xl border px-3 py-4 transition-colors ${
							active
								? "border-accent bg-accent/10 text-ink"
								: "border-app-line text-ink-dull hover:bg-app-box/50 hover:text-ink"
						}`}
					>
						<Icon
							className={`size-5 ${active ? "text-accent" : ""}`}
							weight={active ? "fill" : "regular"}
						/>
						<span className="text-sm font-medium">{label}</span>
					</button>
				);
			})}
		</div>
	);
}
