import { HourglassIcon, TableIcon, ZapIcon } from "lucide-react";
import * as React from "react";
import { formatStat, Panel } from "@/components/catalog/detail-layout";
import { EntityImage } from "@/components/catalog/entity-image";
import { LevelPicker } from "@/components/catalog/level-control";
import { RichText } from "@/components/rich-text/rich-text";
import { Badge } from "@/components/ui/badge";
import { Toggle } from "@/components/ui/toggle";
import type { Skill, SkillLevel } from "@/lib/catalog";
import { cn } from "@/lib/utils";

export const SKILL_TYPE_LABELS: Record<string, string> = {
	NormalAttack: "Basic Attack",
	NormalSkill: "Battle Skill",
	ComboSkill: "Combo Skill",
	UltimateSkill: "Ultimate",
};

const COST_LABELS: Record<string, string> = {
	Atb: "SP",
	UltimateSp: "Energy",
};

interface StatRow {
	name: string;
	values: (string | undefined)[];
}

function blackboardLabel(key: string): string {
	return key.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

/** One row per stat, one column per level (stats, else raw blackboard values). */
function statRows(levels: SkillLevel[]): StatRow[] {
	const rows = new Map<string, (string | undefined)[]>();
	const put = (name: string, i: number, value: string | undefined) => {
		let row = rows.get(name);
		if (!row) {
			row = Array.from({ length: levels.length }, () => undefined);
			rows.set(name, row);
		}
		row[i] = value;
	};
	const hasStats = levels.some((l) => l.stats && l.stats.length > 0);
	levels.forEach((level, i) => {
		if (hasStats) {
			for (const stat of level.stats ?? []) put(stat.name, i, stat.value);
		} else {
			for (const [key, value] of Object.entries(level.blackboard ?? {})) {
				put(blackboardLabel(key), i, formatStat(value));
			}
		}
	});
	levels.forEach((level, i) => {
		if (level.cost) {
			put(
				`Cost (${COST_LABELS[level.cost.type] ?? level.cost.type})`,
				i,
				formatStat(level.cost.value),
			);
		}
		if (level.cooldown) put("Cooldown (s)", i, formatStat(level.cooldown));
	});
	return [...rows].map(([name, values]) => ({ name, values }));
}

/**
 * Skill cards with one shared level selector. "Compare" swaps the per-level
 * stat list for a table across every level.
 */
export function SkillsPanel({
	skills,
	typeLabel,
}: {
	skills: Skill[];
	/** Label above the name (e.g. skill type); defaults to the type label. */
	typeLabel?: (skill: Skill, index: number) => string | undefined;
}): React.ReactElement {
	const maxLevels = Math.max(1, ...skills.map((s) => s.levels.length));
	const allLevels = React.useMemo(
		() => Array.from({ length: maxLevels }, (_, i) => i + 1),
		[maxLevels],
	);
	const [level, setLevel] = React.useState(maxLevels);
	const [compare, setCompare] = React.useState(false);

	return (
		<div className="flex flex-col gap-4">
			<div className="sticky top-0 z-10 -mx-1 flex flex-wrap items-center gap-3 rounded-xl border bg-background/88 px-3 py-2 shadow-xs/5 backdrop-blur-md">
				<span className="text-muted-foreground text-sm">Level</span>
				<div className="-my-1 min-w-0 flex-1 overflow-x-auto py-1">
					<LevelPicker levels={allLevels} onChange={setLevel} value={level} />
				</div>
				<Toggle
					aria-label="Compare all levels"
					onPressedChange={setCompare}
					pressed={compare}
					size="sm"
					variant="outline"
				>
					<TableIcon />
					Compare
				</Toggle>
			</div>
			{skills.map((skill, i) => (
				<SkillCard
					compare={compare}
					key={skill.id}
					level={level}
					onLevelChange={setLevel}
					skill={skill}
					typeLabel={
						typeLabel?.(skill, i) ??
						(skill.type ? SKILL_TYPE_LABELS[skill.type] : undefined)
					}
				/>
			))}
		</div>
	);
}

function SkillCard({
	skill,
	level,
	compare,
	typeLabel,
	onLevelChange,
}: {
	skill: Skill;
	level: number;
	compare: boolean;
	typeLabel?: string;
	onLevelChange: (level: number) => void;
}): React.ReactElement {
	const index = Math.min(level, skill.levels.length) - 1;
	const current = skill.levels[index];
	const previous = index > 0 ? skill.levels[index - 1] : undefined;
	const rows = React.useMemo(() => statRows(skill.levels), [skill.levels]);
	const prevValue = (name: string) =>
		rows.find((r) => r.name === name)?.values[index - 1];

	return (
		<Panel className="flex flex-col gap-3">
			<div className="flex items-start gap-3">
				{skill.icon && (
					<EntityImage
						className="size-12 shrink-0 rounded-lg bg-neutral-900 p-1.5 ring-1 ring-black/5 dark:bg-neutral-800"
						path={skill.icon}
						size={128}
					/>
				)}
				<div className="min-w-0 flex-1">
					{typeLabel && (
						<div className="text-muted-foreground text-xs uppercase tracking-wide">
							{typeLabel}
						</div>
					)}
					<h3 className="font-heading font-semibold text-base">{skill.name}</h3>
				</div>
				<div className="flex shrink-0 flex-wrap justify-end gap-1.5">
					<Badge className="tabular-nums" variant="outline">
						Lv {current?.level ?? level}
					</Badge>
					{current?.cost && (
						<Badge variant="info">
							<ZapIcon />
							{formatStat(current.cost.value)}{" "}
							{COST_LABELS[current.cost.type] ?? current.cost.type}
						</Badge>
					)}
					{current?.cooldown ? (
						<Badge variant="secondary">
							<HourglassIcon />
							{formatStat(current.cooldown)}s
						</Badge>
					) : null}
				</div>
			</div>
			{current && (
				<RichText
					as="p"
					className="text-pretty text-foreground/90 text-sm leading-relaxed"
					value={current.description}
				/>
			)}
			{rows.length > 0 &&
				(compare ? (
					<div className="-mx-4 overflow-x-auto px-4">
						<table className="w-full border-separate border-spacing-0 text-xs">
							<thead>
								<tr>
									<th className="sticky left-0 z-[1] bg-card py-1.5 pe-3 text-start font-medium text-muted-foreground">
										Stat
									</th>
									{skill.levels.map((l, i) => (
										<th
											className={cn(
												"cursor-pointer px-2 py-1.5 text-end font-medium tabular-nums",
												i === index
													? "rounded-t-md bg-accent text-foreground"
													: "text-muted-foreground hover:text-foreground",
											)}
											key={l.level}
											onClick={() => onLevelChange(l.level)}
											scope="col"
										>
											{l.level}
										</th>
									))}
								</tr>
							</thead>
							<tbody>
								{rows.map((row) => (
									<tr className="[&>td]:border-t" key={row.name}>
										<th
											className="sticky left-0 z-[1] whitespace-nowrap bg-card py-1.5 pe-3 text-start font-normal text-muted-foreground"
											scope="row"
										>
											{row.name}
										</th>
										{row.values.map((value, i) => (
											<td
												className={cn(
													"whitespace-nowrap px-2 py-1.5 text-end tabular-nums",
													i === index && "bg-accent font-medium",
													i > 0 && value !== row.values[i - 1]
														? "text-foreground"
														: "text-foreground/60",
												)}
												// biome-ignore lint/suspicious/noArrayIndexKey: level columns
												key={i}
											>
												{value ?? "—"}
											</td>
										))}
									</tr>
								))}
							</tbody>
						</table>
					</div>
				) : (
					<dl className="grid grid-cols-[repeat(auto-fill,minmax(11rem,1fr))] gap-2">
						{rows.map((row) => {
							const value = row.values[index];
							const before = previous ? prevValue(row.name) : undefined;
							const changed = before !== undefined && before !== value;
							return (
								<div
									className="flex flex-col rounded-lg bg-muted/60 px-3 py-2"
									key={row.name}
								>
									<dt
										className="truncate text-muted-foreground text-xs"
										title={row.name}
									>
										{row.name}
									</dt>
									<dd className="flex items-baseline gap-1.5 font-medium text-sm tabular-nums">
										{value ?? "—"}
										{changed && (
											<span className="font-normal text-emerald-700 text-xs dark:text-emerald-400">
												from {before}
											</span>
										)}
									</dd>
								</div>
							);
						})}
					</dl>
				))}
		</Panel>
	);
}
