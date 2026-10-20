/** Flyout bodies: parts, background and lighting. */
import { CheckIcon, EyeIcon, EyeOffIcon } from "lucide-react";
import * as React from "react";
import type { LoadedModel, PartStatus } from "@/components/studio/load-model";
import {
	BACKGROUND_IDS,
	type BackgroundId,
	LIGHTING_IDS,
	type LightingId,
	type ShadingId,
	type StudioPrefs,
	setPrefs,
} from "@/components/studio/prefs";
import { BACKGROUNDS, LIGHTING } from "@/components/studio/presets";
import { PanelHeading } from "@/components/studio/ui/chrome";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

// ------------------------------------------------------------- parts

const STATUS_LABEL: Record<PartStatus, string> = {
	textured: "Textured",
	shared: "Shared texture",
	neutral: "No texture",
	embedded: "Embedded",
};

export function PartsPanel({
	model,
	hidden,
	onHiddenChange,
}: {
	model: LoadedModel | null;
	hidden: ReadonlySet<string>;
	onHiddenChange: (next: Set<string>) => void;
}): React.ReactElement {
	if (!model) {
		return (
			<p className="w-72 py-6 text-center text-muted-foreground text-sm">
				Load a model to see its parts.
			</p>
		);
	}
	const visible =
		model.parts.length - model.parts.filter((p) => hidden.has(p.id)).length;
	const toggle = (id: string, on: boolean) => {
		const next = new Set(hidden);
		if (on) next.delete(id);
		else next.add(id);
		onHiddenChange(next);
	};
	return (
		<div className="flex w-full flex-col gap-2 sm:w-80">
			<PanelHeading
				action={
					<div className="flex gap-1">
						<Button
							onClick={() => onHiddenChange(new Set())}
							size="xs"
							variant="ghost"
						>
							<EyeIcon aria-hidden="true" />
							Show all
						</Button>
						<Button
							onClick={() =>
								onHiddenChange(
									new Set(
										model.parts
											.filter((p) => p.hiddenByDefault)
											.map((p) => p.id),
									),
								)
							}
							size="xs"
							variant="ghost"
						>
							Reset
						</Button>
					</div>
				}
			>
				{visible} of {model.parts.length} parts
			</PanelHeading>
			<ul className="-mx-1 flex max-h-[min(24rem,50dvh)] flex-col overflow-y-auto">
				{model.parts.map((part) => {
					const on = !hidden.has(part.id);
					const id = `part-${part.id}`;
					return (
						<li key={part.id}>
							<label
								className={cn(
									"flex cursor-pointer items-center gap-2.5 rounded-lg px-2 py-1.5 hover:bg-accent/60",
									!on && "text-muted-foreground",
								)}
								htmlFor={id}
								title={part.path ?? part.label}
							>
								<Checkbox
									checked={on}
									id={id}
									onCheckedChange={(checked) => toggle(part.id, checked)}
								/>
								<span className="min-w-0 flex-1 truncate text-sm capitalize">
									{part.label}
								</span>
								{part.lod !== undefined && part.lod > 0 && (
									<span className="font-mono text-[0.625rem] text-muted-foreground">
										lod{part.lod}
									</span>
								)}
								<span
									className={cn(
										"size-2 shrink-0 rounded-full",
										part.status === "textured" || part.status === "embedded"
											? "bg-emerald-500"
											: part.status === "shared"
												? "bg-amber-500"
												: "bg-muted-foreground/40",
									)}
									title={STATUS_LABEL[part.status]}
								>
									<span className="sr-only">{STATUS_LABEL[part.status]}</span>
								</span>
								{!on && (
									<EyeOffIcon
										aria-hidden="true"
										className="size-3.5 shrink-0"
									/>
								)}
							</label>
						</li>
					);
				})}
			</ul>
			<div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t pt-2 text-[0.6875rem] text-muted-foreground">
				<span className="flex items-center gap-1">
					<span className="size-2 rounded-full bg-emerald-500" /> Own texture
				</span>
				<span className="flex items-center gap-1">
					<span className="size-2 rounded-full bg-amber-500" /> Borrowed
				</span>
				<span className="flex items-center gap-1">
					<span className="size-2 rounded-full bg-muted-foreground/40" />{" "}
					Untextured
				</span>
				<span className="ms-auto tabular-nums">
					{formatNumber(model.stats.triangles)} tris
				</span>
			</div>
		</div>
	);
}

// ------------------------------------------------------------- radios

/**
 * Roving focus for a custom radiogroup: only the checked option is tabbable
 * (so a flyout's initial focus lands on the current choice), and the arrow
 * keys move between options and select them.
 */
function onRadioKeyDown(
	e: React.KeyboardEvent<HTMLElement>,
	columns = 1,
): void {
	const radios = [
		...e.currentTarget.querySelectorAll<HTMLElement>('[role="radio"]'),
	];
	const index = radios.indexOf(document.activeElement as HTMLElement);
	if (index < 0) return;
	const delta =
		e.key === "ArrowRight"
			? 1
			: e.key === "ArrowLeft"
				? -1
				: e.key === "ArrowDown"
					? columns
					: e.key === "ArrowUp"
						? -columns
						: 0;
	if (!delta) return;
	e.preventDefault();
	const next = radios[(index + delta + radios.length) % radios.length];
	next?.focus();
	next?.click();
}

// ------------------------------------------------------------- background

export function BackgroundPanel({
	prefs,
	current,
	onSelect,
}: {
	prefs: StudioPrefs;
	/** The effective choice (URL override or saved). */
	current: BackgroundId;
	onSelect: (id: BackgroundId, color?: string) => void;
}): React.ReactElement {
	const colorRef = React.useRef<HTMLInputElement>(null);
	return (
		<div className="flex w-full flex-col gap-3 sm:w-72">
			<PanelHeading>Backdrop</PanelHeading>
			<div
				aria-label="Backdrop"
				className="grid grid-cols-4 gap-2 sm:grid-cols-4"
				onKeyDown={(e) => onRadioKeyDown(e, 4)}
				role="radiogroup"
			>
				{BACKGROUND_IDS.map((id) => {
					const def = BACKGROUNDS[id];
					const checked = current === id;
					return (
						<button
							aria-checked={checked}
							aria-label={def.label}
							className="group flex flex-col items-center gap-1 rounded-lg p-1 text-[0.6875rem] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
							key={id}
							onClick={(e) => {
								onSelect(id);
								// Only a real click opens the colour picker, not arrow keys.
								if (id === "custom" && e.detail > 0) colorRef.current?.click();
							}}
							role="radio"
							tabIndex={checked ? 0 : -1}
							type="button"
						>
							<span
								className={cn(
									"relative flex aspect-square w-full items-center justify-center rounded-md border shadow-xs/5",
									checked &&
										"ring-2 ring-primary ring-offset-2 ring-offset-popover",
								)}
								style={{
									background:
										id === "custom" && checked ? prefs.customColor : def.swatch,
								}}
							>
								{checked && (
									<CheckIcon
										aria-hidden="true"
										className="size-4 text-white drop-shadow-[0_1px_1px_rgb(0_0_0/0.6)]"
									/>
								)}
							</span>
							<span className="w-full truncate text-center">{def.label}</span>
						</button>
					);
				})}
			</div>
			<label className="flex items-center justify-between gap-2 text-sm">
				<span>Custom colour</span>
				<span className="flex items-center gap-2">
					<span className="font-mono text-muted-foreground text-xs uppercase">
						{prefs.customColor}
					</span>
					<input
						aria-label="Custom backdrop colour"
						className="size-7 cursor-pointer rounded-md border bg-transparent p-0.5"
						onChange={(e) => onSelect("custom", e.target.value)}
						ref={colorRef}
						type="color"
						value={prefs.customColor}
					/>
				</span>
			</label>
			<label className="flex items-center justify-between gap-2 text-sm">
				<span>Floor disc</span>
				<Switch
					checked={prefs.floor}
					onCheckedChange={(floor) => setPrefs({ floor })}
				/>
			</label>
			<label className="flex items-center justify-between gap-2 text-sm">
				<span>Floor grid</span>
				<Switch
					checked={prefs.grid}
					onCheckedChange={(grid) => setPrefs({ grid })}
				/>
			</label>
		</div>
	);
}

// ------------------------------------------------------------- lighting

export function LightingPanel({
	prefs,
}: {
	prefs: StudioPrefs;
}): React.ReactElement {
	return (
		<div className="flex w-full flex-col gap-3 sm:w-72">
			<div className="flex items-center justify-between gap-3">
				<span className="flex flex-col">
					<span className="font-medium text-sm" id="shading-label">
						Shading
					</span>
					<span className="text-muted-foreground text-xs">
						{prefs.shading === "toon"
							? "Cel bands and ink outlines"
							: "Smooth physically based light"}
					</span>
				</span>
				<ToggleGroup
					aria-labelledby="shading-label"
					onValueChange={(v) => {
						const next = v[0] as ShadingId | undefined;
						if (next) setPrefs({ shading: next });
					}}
					size="sm"
					value={[prefs.shading]}
					variant="outline"
				>
					<ToggleGroupItem value="toon">Toon</ToggleGroupItem>
					<ToggleGroupItem value="standard">Standard</ToggleGroupItem>
				</ToggleGroup>
			</div>
			<label
				className={cn(
					"flex items-center justify-between gap-2 text-sm",
					prefs.shading !== "toon" && "text-muted-foreground",
				)}
			>
				<span>Outlines</span>
				<Switch
					checked={prefs.outlines && prefs.shading === "toon"}
					disabled={prefs.shading !== "toon"}
					onCheckedChange={(outlines) => setPrefs({ outlines })}
				/>
			</label>
			<PanelHeading>Lighting</PanelHeading>
			<div
				aria-label="Lighting preset"
				className="flex flex-col gap-1"
				onKeyDown={(e) => onRadioKeyDown(e)}
				role="radiogroup"
			>
				{LIGHTING_IDS.map((id: LightingId) => {
					const preset = LIGHTING[id];
					const checked = prefs.lighting === id;
					return (
						<button
							aria-checked={checked}
							className={cn(
								"flex items-center gap-3 rounded-lg border px-3 py-2 text-start outline-none transition-colors hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-ring",
								checked
									? "border-primary/50 bg-primary/8"
									: "border-transparent",
							)}
							key={id}
							onClick={() => setPrefs({ lighting: id })}
							role="radio"
							tabIndex={checked ? 0 : -1}
							type="button"
						>
							<span className="flex min-w-0 flex-1 flex-col">
								<span className="font-medium text-sm">{preset.label}</span>
								<span className="truncate text-muted-foreground text-xs">
									{preset.description}
								</span>
							</span>
							{checked && (
								<CheckIcon aria-hidden="true" className="size-4 text-primary" />
							)}
						</button>
					);
				})}
			</div>
			<div className="flex flex-col gap-2">
				<div className="flex items-center justify-between text-sm">
					<span id="exposure-label">Exposure</span>
					<Badge className="tabular-nums" size="sm" variant="outline">
						{prefs.exposure.toFixed(2)}
					</Badge>
				</div>
				<Slider
					aria-labelledby="exposure-label"
					max={2}
					min={0.4}
					onValueChange={(v) =>
						setPrefs({ exposure: Array.isArray(v) ? v[0] : v })
					}
					step={0.05}
					value={prefs.exposure}
				/>
			</div>
			<label className="flex items-center justify-between gap-2 text-sm">
				<span>Ground shadow</span>
				<Switch
					checked={prefs.groundShadow}
					onCheckedChange={(groundShadow) => setPrefs({ groundShadow })}
				/>
			</label>
		</div>
	);
}
