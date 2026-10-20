import { MinusIcon, PlusIcon } from "lucide-react";
import type * as React from "react";
import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";

/** Level slider with −/+ steppers; the slider thumb is keyboard accessible. */
export function LevelSlider({
	value,
	min,
	max,
	onChange,
	label = "Level",
	className,
}: {
	value: number;
	min: number;
	max: number;
	onChange: (value: number) => void;
	label?: string;
	className?: string;
}): React.ReactElement {
	const clamp = (v: number) => Math.max(min, Math.min(max, v));
	return (
		<div className={cn("flex items-center gap-3", className)}>
			<div className="flex w-20 shrink-0 flex-col leading-tight">
				<span className="text-muted-foreground text-xs">{label}</span>
				<span className="font-heading font-semibold text-xl tabular-nums">
					{value}
					<span className="font-normal text-muted-foreground text-xs">
						{" "}
						/ {max}
					</span>
				</span>
			</div>
			<Button
				aria-label={`Decrease ${label.toLowerCase()}`}
				disabled={value <= min}
				onClick={() => onChange(clamp(value - 1))}
				size="icon-sm"
				variant="outline"
			>
				<MinusIcon />
			</Button>
			<Slider
				aria-label={label}
				className="min-w-0 flex-1"
				getAriaLabel={() => label}
				max={max}
				min={min}
				onValueChange={(v) => {
					const next = Array.isArray(v) ? v[0] : v;
					if (typeof next === "number") onChange(clamp(Math.round(next)));
				}}
				value={value}
			/>
			<Button
				aria-label={`Increase ${label.toLowerCase()}`}
				disabled={value >= max}
				onClick={() => onChange(clamp(value + 1))}
				size="icon-sm"
				variant="outline"
			>
				<PlusIcon />
			</Button>
		</div>
	);
}

/** Segmented level picker for small ranges (skill levels 1–12). */
export function LevelPicker({
	levels,
	value,
	onChange,
	label = "Skill level",
	format = (l) => String(l),
}: {
	levels: number[];
	value: number;
	onChange: (value: number) => void;
	label?: string;
	format?: (level: number) => string;
}): React.ReactElement {
	return (
		<ToggleGroup
			aria-label={label}
			onValueChange={(next) => {
				const v = next[0];
				if (v !== undefined) onChange(Number(v));
			}}
			size="sm"
			value={[String(value)]}
			variant="outline"
		>
			{levels.map((level) => (
				<ToggleGroupItem
					aria-label={`${label} ${level}`}
					className="min-w-8 tabular-nums"
					key={level}
					value={String(level)}
				>
					{format(level)}
				</ToggleGroupItem>
			))}
		</ToggleGroup>
	);
}
