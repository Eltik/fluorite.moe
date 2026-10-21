/** Settings popover body: display toggles and renderer options. */
import { CircleHelpIcon, MoonIcon, SunIcon } from "lucide-react";
import type * as React from "react";
import {
	DPR_CAPS,
	resetPrefs,
	type StudioPrefs,
	setPrefs,
	type ZUpMode,
} from "@/components/studio/prefs";
import { PanelHeading, ShortcutHint } from "@/components/studio/ui/chrome";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";

function Row({
	label,
	hint,
	shortcut,
	checked,
	disabled,
	onChange,
}: {
	label: string;
	hint?: string;
	shortcut?: string;
	checked: boolean;
	disabled?: boolean;
	onChange: (value: boolean) => void;
}) {
	return (
		<label
			className={cn(
				"flex items-center justify-between gap-3 py-1 text-sm",
				disabled && "text-muted-foreground",
			)}
		>
			<span className="flex min-w-0 flex-col">
				<ShortcutHint label={label} shortcut={shortcut} />
				{hint && <span className="text-muted-foreground text-xs">{hint}</span>}
			</span>
			<Switch
				checked={checked}
				disabled={disabled}
				onCheckedChange={onChange}
			/>
		</label>
	);
}

export function SettingsPanel({
	prefs,
	rigged,
	hasModel,
	detectedZUp,
	compact,
	themeDark,
	onHelp,
	onToggleTheme,
}: {
	prefs: StudioPrefs;
	rigged: boolean;
	hasModel: boolean;
	detectedZUp: boolean;
	/** Phones: also offers help and the theme switch (hidden from the top bar). */
	compact: boolean;
	themeDark: boolean;
	onHelp: () => void;
	onToggleTheme: () => void;
}): React.ReactElement {
	return (
		<div className="flex max-h-[calc(100dvh-5.5rem)] w-72 max-w-[calc(100vw-2rem)] flex-col gap-3 overflow-y-auto">
			{compact && (
				<div className="flex gap-2">
					<Button
						className="flex-1"
						onClick={onHelp}
						size="sm"
						variant="outline"
					>
						<CircleHelpIcon aria-hidden="true" />
						Controls
					</Button>
					<Button
						className="flex-1"
						onClick={onToggleTheme}
						size="sm"
						variant="outline"
					>
						{themeDark ? (
							<SunIcon aria-hidden="true" />
						) : (
							<MoonIcon aria-hidden="true" />
						)}
						{themeDark ? "Light theme" : "Dark theme"}
					</Button>
				</div>
			)}
			<section className="flex flex-col">
				<PanelHeading>Scene</PanelHeading>
				<Row
					checked={prefs.shading === "toon"}
					hint="Cel shading with outlines; off for the standard look"
					label="Toon shading"
					onChange={(toon) => setPrefs({ shading: toon ? "toon" : "standard" })}
					shortcut="T"
				/>
				<Row
					checked={prefs.floor}
					label="Floor disc"
					onChange={(floor) => setPrefs({ floor })}
				/>
				<Row
					checked={prefs.grid}
					label="Floor grid"
					onChange={(grid) => setPrefs({ grid })}
					shortcut="G"
				/>
				<Row
					checked={prefs.groundShadow}
					label="Ground shadow"
					onChange={(groundShadow) => setPrefs({ groundShadow })}
				/>
				<Row
					checked={prefs.autoRotate}
					label="Auto-rotate"
					onChange={(autoRotate) => setPrefs({ autoRotate })}
					shortcut="O"
				/>
			</section>
			<section className="flex flex-col">
				<PanelHeading>Toon look</PanelHeading>
				<Row
					checked={prefs.lightFollow}
					disabled={prefs.shading !== "toon"}
					hint="Key light orbits with the view, as in the game menu"
					label="Light follows camera"
					onChange={(lightFollow) => setPrefs({ lightFollow })}
				/>
				<Row
					checked={prefs.extraLights}
					disabled={prefs.shading !== "toon"}
					hint="The character's own fill and rim lights"
					label="Extra lights"
					onChange={(extraLights) => setPrefs({ extraLights })}
				/>
				<Row
					checked={prefs.selfShadow}
					disabled={prefs.shading !== "toon"}
					hint="Cast onto the character itself and the floor"
					label="Shadows"
					onChange={(selfShadow) => setPrefs({ selfShadow })}
				/>
				<Row
					checked={prefs.bloom}
					disabled={prefs.shading !== "toon"}
					hint="Soft glow on highlights"
					label="Bloom"
					onChange={(bloom) => setPrefs({ bloom })}
				/>
			</section>
			<section className="flex flex-col">
				<PanelHeading>Debug</PanelHeading>
				<p className="pb-1 text-muted-foreground text-xs">
					Helpers, normals, surface views and mesh inspection live in the View
					panel (V).
				</p>
				<Row
					checked={prefs.wireframe}
					label="Wireframe"
					onChange={(wireframe) => setPrefs({ wireframe })}
					shortcut="W"
				/>
				<Row
					checked={prefs.skeleton && rigged}
					disabled={!rigged}
					hint={rigged ? undefined : "Only rigged models have bones"}
					label="Skeleton"
					onChange={(skeleton) => setPrefs({ skeleton })}
					shortcut="K"
				/>
				<Row
					checked={prefs.stats}
					label="Performance stats"
					onChange={(stats) => setPrefs({ stats })}
					shortcut="⇧S"
				/>
			</section>
			<section className="flex flex-col gap-2">
				<PanelHeading>Orientation</PanelHeading>
				<div className="flex items-center justify-between gap-3 text-sm">
					<span className="flex flex-col">
						<span>Z-up correction</span>
						<span className="text-muted-foreground text-xs">
							{prefs.zUp === "auto"
								? hasModel
									? detectedZUp
										? "Auto: rotated upright"
										: "Auto: already upright"
									: "Auto-detected from bounds"
								: prefs.zUp === "on"
									? "Always rotate −90° on X"
									: "Use file orientation"}
						</span>
					</span>
					<ToggleGroup
						aria-label="Z-up correction"
						onValueChange={(v) => {
							const next = v[0] as ZUpMode | undefined;
							if (next) setPrefs({ zUp: next });
						}}
						size="sm"
						value={[prefs.zUp]}
						variant="outline"
					>
						<ToggleGroupItem value="auto">Auto</ToggleGroupItem>
						<ToggleGroupItem value="on">On</ToggleGroupItem>
						<ToggleGroupItem value="off">Off</ToggleGroupItem>
					</ToggleGroup>
				</div>
			</section>
			<section className="flex flex-col gap-2">
				<PanelHeading>Rendering</PanelHeading>
				<div className="flex items-center justify-between gap-3 text-sm">
					<span className="flex flex-col">
						<span>Pixel ratio cap</span>
						<span className="text-muted-foreground text-xs">
							Lower is faster
						</span>
					</span>
					<ToggleGroup
						aria-label="Pixel ratio cap"
						onValueChange={(v) => {
							const next = Number(v[0]);
							if (next) setPrefs({ dprCap: next });
						}}
						size="sm"
						value={[String(prefs.dprCap)]}
						variant="outline"
					>
						{DPR_CAPS.map((cap) => (
							<ToggleGroupItem key={cap} value={String(cap)}>
								{cap}×
							</ToggleGroupItem>
						))}
					</ToggleGroup>
				</div>
			</section>
			<Button
				className="self-start"
				onClick={resetPrefs}
				size="sm"
				variant="outline"
			>
				Reset settings
			</Button>
		</div>
	);
}
