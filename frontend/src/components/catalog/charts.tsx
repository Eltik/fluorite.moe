import * as React from "react";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

const W = 640;
const H = 200;
const PAD = { top: 26, right: 16, bottom: 28, left: 52 };

function niceTicks(min: number, max: number, count = 4): number[] {
	if (min === max) return [min];
	const span = max - min;
	const step0 = span / count;
	const mag = 10 ** Math.floor(Math.log10(step0));
	const norm = step0 / mag;
	const step = (norm >= 5 ? 10 : norm >= 2 ? 5 : norm >= 1 ? 2 : 1) * mag;
	const start = Math.ceil(min / step) * step;
	const ticks: number[] = [];
	for (let v = start; v <= max + step * 1e-6; v += step) ticks.push(v);
	return ticks;
}

export function compactNumber(v: number): string {
	const abs = Math.abs(v);
	if (abs >= 1e6) return `${+(v / 1e6).toFixed(1)}M`;
	if (abs >= 1e4) return `${+(v / 1e3).toFixed(1)}k`;
	if (Number.isInteger(v)) return formatNumber(v);
	return String(+v.toFixed(2));
}

/**
 * Small, dependency-free line chart. `x` labels the horizontal axis (e.g.
 * levels); pointer movement scrubs `onSelect`. `markers` draw dashed guides at
 * indices (break-stage boundaries).
 */
export function LineChart({
	x,
	y,
	cursor,
	onSelect,
	markers = [],
	markerLabels,
	label,
	formatValue = compactNumber,
	formatX = (v) => `Lv ${v}`,
	className,
}: {
	x: number[];
	y: number[];
	cursor?: number;
	onSelect?: (index: number) => void;
	markers?: number[];
	/** Short labels drawn at the top of each marker (same order as `markers`). */
	markerLabels?: string[];
	/** Label of an x value in the hover readout. */
	formatX?: (v: number) => string;
	label: string;
	formatValue?: (v: number) => string;
	className?: string;
}): React.ReactElement {
	const n = y.length;
	const finite = y.filter(Number.isFinite);
	let min = finite.length ? Math.min(...finite) : 0;
	let max = finite.length ? Math.max(...finite) : 1;
	if (min === max) {
		min -= 1;
		max += 1;
	} else if (min > 0 && min / max < 0.35) {
		min = 0;
	}
	const innerW = W - PAD.left - PAD.right;
	const innerH = H - PAD.top - PAD.bottom;
	const px = (i: number) =>
		PAD.left + (n <= 1 ? innerW / 2 : (i / (n - 1)) * innerW);
	const py = (v: number) =>
		PAD.top + innerH - ((v - min) / (max - min)) * innerH;

	const d = y
		.map(
			(v, i) => `${i === 0 ? "M" : "L"}${px(i).toFixed(1)},${py(v).toFixed(1)}`,
		)
		.join("");
	const area = `${d}L${px(n - 1).toFixed(1)},${PAD.top + innerH}L${px(0).toFixed(1)},${PAD.top + innerH}Z`;
	const ticks = niceTicks(min, max);
	const xTickIdx = React.useMemo(() => {
		if (n === 0) return [];
		const want = 6;
		const out = new Set<number>([0, n - 1]);
		for (let k = 1; k < want - 1; k++)
			out.add(Math.round((k / (want - 1)) * (n - 1)));
		return [...out].sort((a, b) => a - b);
	}, [n]);

	const gradientId = React.useId();
	const svgRef = React.useRef<SVGSVGElement>(null);
	const scrub = (event: React.PointerEvent) => {
		if (!onSelect || !svgRef.current || n === 0) return;
		const rect = svgRef.current.getBoundingClientRect();
		const sx = ((event.clientX - rect.left) / rect.width) * W;
		const t = (sx - PAD.left) / innerW;
		onSelect(Math.max(0, Math.min(n - 1, Math.round(t * (n - 1)))));
	};

	const cx =
		cursor !== undefined && cursor >= 0 && cursor < n ? px(cursor) : null;
	const cy =
		cursor !== undefined && y[cursor] !== undefined
			? py(y[cursor] as number)
			: null;

	return (
		<svg
			aria-label={label}
			className={cn(
				"h-auto w-full touch-pan-y select-none text-blue-600 dark:text-blue-400",
				className,
			)}
			onPointerDown={(e) => {
				if (e.pointerType !== "mouse") scrub(e);
			}}
			onPointerMove={(e) => {
				if (e.pointerType === "mouse" ? true : e.buttons > 0) scrub(e);
			}}
			ref={svgRef}
			role="img"
			viewBox={`0 0 ${W} ${H}`}
		>
			<defs>
				<linearGradient id={gradientId} x1="0" x2="0" y1="0" y2="1">
					<stop offset="0%" stopColor="currentColor" stopOpacity="0.22" />
					<stop offset="100%" stopColor="currentColor" stopOpacity="0" />
				</linearGradient>
			</defs>
			{ticks.map((t) => (
				<g key={t}>
					<line
						className="stroke-border/70"
						x1={PAD.left}
						x2={W - PAD.right}
						y1={py(t)}
						y2={py(t)}
					/>
					<text
						className="fill-foreground/70 text-[12px] tabular-nums"
						dominantBaseline="middle"
						textAnchor="end"
						x={PAD.left - 8}
						y={py(t)}
					>
						{formatValue(t)}
					</text>
				</g>
			))}
			{markers.map((m, i) => (
				<g key={m}>
					<line
						className="stroke-foreground/35"
						strokeDasharray="3 4"
						x1={px(m)}
						x2={px(m)}
						y1={PAD.top - 4}
						y2={PAD.top + innerH}
					/>
					{markerLabels?.[i] && (
						<text
							className="fill-foreground/70 text-[11px] tabular-nums"
							textAnchor="middle"
							x={px(m)}
							y={PAD.top - 9}
						>
							{markerLabels[i]}
						</text>
					)}
				</g>
			))}
			{xTickIdx.map((i) => (
				<text
					className="fill-foreground/70 text-[12px] tabular-nums"
					key={i}
					textAnchor={i === 0 ? "start" : i === n - 1 ? "end" : "middle"}
					x={px(i)}
					y={H - 8}
				>
					{x[i]}
				</text>
			))}
			<path d={area} fill={`url(#${gradientId})`} />
			<path
				d={d}
				fill="none"
				stroke="currentColor"
				strokeLinejoin="round"
				strokeWidth={2}
				vectorEffect="non-scaling-stroke"
			/>
			{cx !== null && cy !== null && (
				<g>
					<line
						className="stroke-foreground/30"
						x1={cx}
						x2={cx}
						y1={PAD.top}
						y2={PAD.top + innerH}
					/>
					<circle
						className="fill-background"
						cx={cx}
						cy={cy}
						r={5}
						stroke="currentColor"
						strokeWidth={2.5}
					/>
					{(() => {
						const text = `${formatX(x[cursor as number] as number)} · ${formatValue(y[cursor as number] as number)}`;
						const w = text.length * 6.6 + 16;
						const left = Math.max(
							PAD.left,
							Math.min(W - PAD.right - w, cx - w / 2),
						);
						const top = cy - 34 < PAD.top - 2 ? cy + 12 : cy - 34;
						return (
							<g aria-hidden pointerEvents="none">
								<rect
									className="fill-popover stroke-border"
									height={22}
									rx={6}
									width={w}
									x={left}
									y={top}
								/>
								<text
									className="fill-popover-foreground font-medium text-[12px] tabular-nums"
									dominantBaseline="central"
									textAnchor="middle"
									x={left + w / 2}
									y={top + 11}
								>
									{text}
								</text>
							</g>
						);
					})()}
				</g>
			)}
		</svg>
	);
}

/** Inline sparkline with an optional cursor dot. */
export function Sparkline({
	values,
	cursor,
	className,
}: {
	values: number[];
	cursor?: number;
	className?: string;
}): React.ReactElement {
	const w = 100;
	const h = 24;
	const n = values.length;
	const min = Math.min(...values);
	const max = Math.max(...values);
	const span = max - min || 1;
	const px = (i: number) => (n <= 1 ? w / 2 : (i / (n - 1)) * w);
	const py = (v: number) => h - 2 - ((v - min) / span) * (h - 4);
	const d = values
		.map(
			(v, i) => `${i === 0 ? "M" : "L"}${px(i).toFixed(2)},${py(v).toFixed(2)}`,
		)
		.join("");
	return (
		<svg
			aria-hidden="true"
			className={cn(
				"h-6 w-24 overflow-visible text-blue-600 dark:text-blue-400",
				className,
			)}
			preserveAspectRatio="none"
			viewBox={`0 0 ${w} ${h}`}
		>
			<path
				d={d}
				fill="none"
				stroke="currentColor"
				strokeOpacity={0.8}
				strokeWidth={1.5}
				vectorEffect="non-scaling-stroke"
			/>
			{cursor !== undefined && values[cursor] !== undefined && (
				<line
					stroke="currentColor"
					strokeOpacity={0.6}
					strokeWidth={1}
					vectorEffect="non-scaling-stroke"
					x1={px(cursor)}
					x2={px(cursor)}
					y1={0}
					y2={h}
				/>
			)}
		</svg>
	);
}
