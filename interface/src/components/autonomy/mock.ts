// Mock data for the WakesCard design preview. Wake definitions don't exist
// in the backend yet; everything else on the autonomy panel reads real
// endpoints via api/client.

import type {AutonomyLevel} from "@/api/client";

export type WakeTriggerKind = "schedule" | "webhook" | "event" | "condition";

export interface WakeDef {
	id: string;
	name: string;
	trigger_kind: WakeTriggerKind;
	trigger_label: string;
	instructions: string;
	min_level: Exclude<AutonomyLevel, "off">;
	builtin: boolean;
	enabled: boolean;
	last_fired_at: string | null;
}

function minutesAgo(n: number): string {
	return new Date(Date.now() - n * 60_000).toISOString();
}

const wakes: WakeDef[] = [
	{
		id: "wake-1",
		name: "Idle survey",
		trigger_kind: "schedule",
		trigger_label: "every 30m",
		instructions:
			"Survey task state, enrich pending proposals, execute approved work.",
		min_level: "observe",
		builtin: true,
		enabled: true,
		last_fired_at: minutesAgo(23),
	},
	{
		id: "wake-2",
		name: "Task approved",
		trigger_kind: "event",
		trigger_label: "on approval",
		instructions: "Start approved work immediately instead of waiting for the next survey.",
		min_level: "act",
		builtin: true,
		enabled: true,
		last_fired_at: minutesAgo(83),
	},
	{
		id: "wake-3",
		name: "Morning brief",
		trigger_kind: "schedule",
		trigger_label: "daily at 8:00",
		instructions:
			"Summarize overnight activity and what needs my attention today. Deliver to my DM.",
		min_level: "observe",
		builtin: false,
		enabled: true,
		last_fired_at: minutesAgo(457),
	},
	{
		id: "wake-4",
		name: "CI failed on main",
		trigger_kind: "webhook",
		trigger_label: "POST /hooks/ci",
		instructions:
			"Investigate the failing job in the payload and propose a fix task with findings.",
		min_level: "suggest",
		builtin: false,
		enabled: true,
		last_fired_at: minutesAgo(2880),
	},
	{
		id: "wake-5",
		name: "Quiet-hours enrichment",
		trigger_kind: "condition",
		trigger_label: "no activity for 2h",
		instructions: "The humans are away. Use the time to research pending proposals.",
		min_level: "suggest",
		builtin: false,
		enabled: false,
		last_fired_at: minutesAgo(113),
	},
];

export const mockAutonomyApi = {
	wakes: (): Promise<{wakes: WakeDef[]}> => Promise.resolve({wakes}),
};
