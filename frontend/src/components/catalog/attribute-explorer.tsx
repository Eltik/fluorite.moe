import * as React from "react";
import {
	compactNumber,
	LineChart,
	Sparkline,
} from "@/components/catalog/charts";
import { formatStat, Panel, Section } from "@/components/catalog/detail-layout";
import { LevelSlider } from "@/components/catalog/level-control";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { type AttrSeries, attrLabel, sortAttrs } from "@/lib/catalog";
import { cn } from "@/lib/utils";

/**
 * Level slider (+ break-stage selector when the series has stages), a chart
 * of the selected attribute and a table of every attribute at that level.
 */
export function AttributeExplorer({
	attributes,
	stageLabel = (s) => (s === 0 ? "Base" : `Stage ${s}`),
	fixed,
	fixedTitle = "Constant attributes",
	markerLevels,
	markerLabel = "promotions",
}: {
	attributes: AttrSeries;
	stageLabel?: (stage: number) => string;
	/** Extra constants to list with the series' own constants. */
	fixed?: Record<string, number>;
	fixedTitle?: string;
	/** Levels to mark on the chart when the series has no break stages. */
	markerLevels?: number[];
	markerLabel?: string;
}): React.ReactElement {
	const { level, breakStage, series, constants } = attributes;
	const n = level.length;
	const hasStages = breakStage.length === n && new Set(breakStage).size > 1;
	const stages = React.useMemo(
		() => (hasStages ? [...new Set(breakStage)].sort((a, b) => a - b) : []),
		[hasStages, breakStage],
	);
	const attrs = React.useMemo(() => sortAttrs(Object.keys(series)), [series]);
	const [row, setRow] = React.useState(Math.max(0, n - 1));
	const [attr, setAttr] = React.useState(attrs[0] ?? "");
	React.useEffect(() => {
		if (row >= n) setRow(Math.max(0, n - 1));
	}, [n, row]);
	React.useEffect(() => {
		if (!attrs.includes(attr)) setAttr(attrs[0] ?? "");
	}, [attrs, attr]);

	const currentLevel = level[row] ?? 1;
	const currentStage = hasStages ? (breakStage[row] ?? 0) : 0;
	const minLevel = level[0] ?? 1;
	const maxLevel = level[n - 1] ?? 1;

	const setLevel = (target: number) => {
		const candidates: number[] = [];
		level.forEach((l, i) => {
			if (l === target) candidates.push(i);
		});
		if (candidates.length === 0) return;
		const same = candidates.find((i) => breakStage[i] === currentStage);
		setRow(same ?? (candidates[0] as number));
	};
	const setStage = (stage: number) => {
		const rows: number[] = [];
		breakStage.forEach((s, i) => {
			if (s === stage) rows.push(i);
		});
		if (rows.length === 0) return;
		const exact = rows.find((i) => level[i] === currentLevel);
		if (exact !== undefined) return setRow(exact);
		const first = rows[0] as number;
		const last = rows[rows.length - 1] as number;
		setRow(currentLevel < (level[first] ?? 0) ? first : last);
	};
	const stageRange = (stage: number) => {
		let lo = Number.POSITIVE_INFINITY;
		let hi = 0;
		breakStage.forEach((s, i) => {
			if (s === stage) {
				lo = Math.min(lo, level[i] ?? lo);
				hi = Math.max(hi, level[i] ?? hi);
			}
		});
		return `Lv ${lo}–${hi}`;
	};
	const markers = React.useMemo(() => {
		if (!hasStages) {
			return (markerLevels ?? [])
				.map((l) => level.indexOf(l))
				.filter((i) => i > 0);
		}
		const out: number[] = [];
		for (let i = 1; i < n; i++)
			if (breakStage[i] !== breakStage[i - 1]) out.push(i);
		return out;
	}, [hasStages, breakStage, n, markerLevels, level]);

	const markerLabels = React.useMemo(
		() =>
			markers.map((i) =>
				hasStages
					? stageLabel(breakStage[i] as number)
					: `Lv ${level[i] as number}`,
			),
		[markers, hasStages, stageLabel, breakStage, level],
	);

	const fixedEntries = React.useMemo(() => {
		const merged = { ...constants, ...(fixed ?? {}) };
		return sortAttrs(Object.keys(merged)).map(
			(k) => [k, merged[k] as number] as const,
		);
	}, [constants, fixed]);

	const values = series[attr] ?? [];

	return (
		<div className="flex flex-col gap-5">
			<Panel className="flex flex-col gap-4">
				<LevelSlider
					max={maxLevel}
					min={minLevel}
					onChange={setLevel}
					value={currentLevel}
				/>
				{hasStages && (
					<div className="flex flex-wrap items-center gap-3">
						<span className="text-muted-foreground text-xs">Promotion</span>
						<ToggleGroup
							aria-label="Break stage"
							onValueChange={(next) => {
								const v = next[0];
								if (v !== undefined) setStage(Number(v));
							}}
							size="sm"
							value={[String(currentStage)]}
							variant="outline"
						>
							{stages.map((stage) => (
								<ToggleGroupItem
									key={stage}
									title={stageRange(stage)}
									value={String(stage)}
								>
									{stageLabel(stage)}
								</ToggleGroupItem>
							))}
						</ToggleGroup>
						<span className="text-muted-foreground text-xs tabular-nums">
							{stageRange(currentStage)}
						</span>
					</div>
				)}
			</Panel>

			{attrs.length > 0 && (
				<div
					className={cn(
						"grid gap-5",
						attrs.length > 1 && "lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]",
					)}
				>
					<Panel className="flex flex-col gap-2">
						<div className="flex items-baseline justify-between gap-2">
							<h3 className="font-medium text-sm">{attrLabel(attr)}</h3>
							<span className="font-heading font-semibold text-lg tabular-nums">
								{values[row] !== undefined
									? formatStat(values[row] as number, attr)
									: "—"}
							</span>
						</div>
						<LineChart
							formatValue={(v) => {
								const text = formatStat(v, attr);
								return text.endsWith("%") ? text : compactNumber(v);
							}}
							label={`${attrLabel(attr)} by level`}
							markerLabels={markerLabels}
							markers={markers}
							onSelect={setRow}
							x={level}
							y={values}
							cursor={row}
						/>
						<p className="text-muted-foreground text-xs">
							Hover or drag the chart to scrub levels.
							{markers.length > 0 && ` Dashed lines mark ${markerLabel}.`}
						</p>
					</Panel>
					<div
						className={cn(
							"overflow-hidden rounded-xl border bg-card",
							attrs.length <= 1 && "hidden",
						)}
					>
						<table className="w-full text-sm">
							<caption className="sr-only">
								Attributes at level {currentLevel}
							</caption>
							<thead className="border-b bg-muted/40 text-muted-foreground text-xs">
								<tr>
									<th className="px-3 py-2 text-start font-medium" scope="col">
										Attribute
									</th>
									<th className="px-3 py-2 text-end font-medium" scope="col">
										Lv {currentLevel}
									</th>
									<th
										className="px-3 py-2 font-medium max-sm:hidden"
										scope="col"
									>
										<span className="sr-only">Trend</span>
									</th>
								</tr>
							</thead>
							<tbody className="divide-y">
								{attrs.map((code) => {
									const vals = series[code] ?? [];
									const active = code === attr;
									return (
										<tr
											className={cn(
												"cursor-pointer transition-colors hover:bg-accent/60",
												active && "bg-accent",
											)}
											key={code}
											onClick={() => setAttr(code)}
										>
											<th
												className="px-3 py-1.5 text-start font-normal"
												scope="row"
											>
												<button
													aria-pressed={active}
													className="cursor-pointer rounded-sm text-start outline-none focus-visible:ring-2 focus-visible:ring-ring"
													onClick={(e) => {
														e.stopPropagation();
														setAttr(code);
													}}
													type="button"
												>
													{attrLabel(code)}
												</button>
											</th>
											<td className="px-3 py-1.5 text-end font-medium tabular-nums">
												{vals[row] !== undefined
													? formatStat(vals[row] as number, code)
													: "—"}
											</td>
											<td className="px-3 py-1.5 max-sm:hidden">
												<Sparkline
													className="ms-auto"
													cursor={row}
													values={vals}
												/>
											</td>
										</tr>
									);
								})}
							</tbody>
						</table>
					</div>
				</div>
			)}

			{fixedEntries.length > 0 && (
				<Section title={fixedTitle}>
					<dl className="grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-2">
						{fixedEntries.map(([code, value]) => (
							<div
								className="flex items-baseline justify-between gap-3 rounded-lg bg-muted/60 px-3 py-2 text-sm"
								key={code}
							>
								<dt className="truncate text-muted-foreground" title={code}>
									{attrLabel(code)}
								</dt>
								<dd className="font-medium tabular-nums">
									{formatStat(value, code)}
								</dd>
							</div>
						))}
					</dl>
				</Section>
			)}
		</div>
	);
}
