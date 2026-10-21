/**
 * The View flyout: camera, display helpers, inspection and capture, in tabs
 * so the toolbar keeps one entry. Persistent toggles write to prefs; modes
 * (pan, pick, normals, selection) live in the session view state.
 */
import {
	CameraIcon,
	CrosshairIcon,
	EyeOffIcon,
	FocusIcon,
	InfoIcon,
	LockIcon,
	MousePointerClickIcon,
	ScanIcon,
	ScanSearchIcon,
	UnlockIcon,
	ZoomInIcon,
	ZoomOutIcon,
} from "lucide-react";
import * as React from "react";
import {
	describeMesh,
	type MeshInfo,
	shortShader,
} from "@/components/studio/inspect";
import type { LoadedModel } from "@/components/studio/load-model";
import {
	type StudioPrefs,
	type SurfaceId,
	setPrefs,
	type ViewTab,
	type ZoomControlMode,
} from "@/components/studio/prefs";
import type { StageHandle } from "@/components/studio/stage";
import { useRenderStats } from "@/components/studio/stats-store";
import { PanelHeading, ShortcutHint } from "@/components/studio/ui/chrome";
import {
	FOV_MAX,
	FOV_MIN,
	VIEW_PRESET_IDS,
	VIEW_PRESET_LABEL,
	type ViewPresetId,
	ZOOM_STEP,
	zoomFromDistance,
} from "@/components/studio/view-math";
import {
	setViewState,
	useCameraReadout,
	type ViewState,
} from "@/components/studio/view-state";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsPanel, TabsTab } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

export interface ViewPanelProps {
	prefs: StudioPrefs;
	view: ViewState;
	model: LoadedModel | null;
	stage: React.RefObject<StageHandle | null>;
	locked: boolean;
	onLock: () => void;
	hiddenParts: ReadonlySet<string>;
	onHiddenChange: (next: Set<string>) => void;
	onCapture: () => void;
	/** Reset the view and the shown parts (F). */
	onReset: () => void;
	/** Frame the camera on the selected mesh (.). */
	onFocus: () => void;
	/** Show only the selected part and frame it (/). */
	onIsolateFocus: () => void;
	/** Phones and tablets: one-finger wording, full-width layout. */
	touch: boolean;
}

/** Switch row shared by every tab. */
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

const PRESET_KEY: Record<ViewPresetId, string> = {
	front: "1",
	back: "2",
	right: "3",
	left: "4",
	top: "5",
	threeQuarter: "6",
};

const ZOOM_MODE_LABEL: Record<ZoomControlMode, string> = {
	auto: "Auto",
	always: "On",
	never: "Off",
};

// ------------------------------------------------------------- camera

function CameraTab({
	prefs,
	view,
	model,
	stage,
	locked,
	onLock,
	onReset,
	touch,
}: ViewPanelProps): React.ReactElement {
	const camera = useCameraReadout();
	const ready = Boolean(model) && !locked;
	const zoom = camera
		? zoomFromDistance(camera.distance, camera.minDistance, camera.maxDistance)
		: 0.5;
	const fov = camera?.fov ?? 20;
	return (
		<div className="flex flex-col gap-3">
			<div className="flex flex-col gap-1.5">
				<div className="flex items-center justify-between text-sm">
					<span id="zoom-label">Zoom</span>
					<Badge className="tabular-nums" size="sm" variant="outline">
						{Math.round(zoom * 100)}%
					</Badge>
				</div>
				<div className="flex items-center gap-1.5">
					<Button
						aria-label="Zoom out"
						disabled={!ready}
						onClick={() => stage.current?.zoomBy(1 / ZOOM_STEP)}
						size="icon-sm"
						variant="outline"
					>
						<ZoomOutIcon />
					</Button>
					<Slider
						aria-labelledby="zoom-label"
						className="min-w-0 flex-1 [&_[data-slot=slider-control]]:min-w-0"
						disabled={!ready}
						getAriaLabel={() => "Zoom"}
						max={1}
						min={0}
						onValueChange={(v) =>
							stage.current?.setZoom(Array.isArray(v) ? v[0] : v)
						}
						step={0.005}
						value={zoom}
					/>
					<Button
						aria-label="Zoom in"
						disabled={!ready}
						onClick={() => stage.current?.zoomBy(ZOOM_STEP)}
						size="icon-sm"
						variant="outline"
					>
						<ZoomInIcon />
					</Button>
				</div>
			</div>
			<Row
				checked={view.panMode}
				hint={
					touch
						? "One finger pans instead of orbiting"
						: "Left-drag pans instead of orbiting"
				}
				label="Pan mode"
				onChange={(panMode) => setViewState({ panMode })}
			/>
			<div className="flex flex-col gap-1.5">
				<span className="text-sm">View</span>
				<div className="grid grid-cols-6 gap-1">
					{VIEW_PRESET_IDS.map((id) => (
						<Button
							aria-label={`${VIEW_PRESET_LABEL[id]} view`}
							className="h-8 px-0 text-xs"
							disabled={!ready}
							key={id}
							onClick={() => stage.current?.setView(id)}
							size="sm"
							title={`${VIEW_PRESET_LABEL[id]} view (${PRESET_KEY[id]})`}
							variant="outline"
						>
							{VIEW_PRESET_LABEL[id]}
						</Button>
					))}
				</div>
			</div>
			<div className="flex flex-col gap-1.5">
				<div className="flex items-center justify-between text-sm">
					<span id="fov-label">Field of view</span>
					<Badge className="tabular-nums" size="sm" variant="outline">
						{Math.round(fov)}°
					</Badge>
				</div>
				<Slider
					aria-labelledby="fov-label"
					disabled={!ready}
					getAriaLabel={() => "Field of view"}
					max={FOV_MAX}
					min={FOV_MIN}
					onValueChange={(v) =>
						stage.current?.setFov(Array.isArray(v) ? v[0] : v)
					}
					step={1}
					value={Math.round(fov)}
				/>
			</div>
			<div className="flex gap-2">
				<Button
					className="flex-1"
					disabled={!model}
					onClick={onReset}
					size="sm"
					title="Back to the default view, parts, weapon and props. Display overlays stay."
					variant="outline"
				>
					<FocusIcon aria-hidden="true" />
					Reset view & parts
					<Kbd className="ms-auto h-4 min-w-4 text-[0.625rem]">F</Kbd>
				</Button>
				<Button
					aria-pressed={locked}
					className={cn("flex-1", locked && "border-primary/50 bg-primary/8")}
					onClick={onLock}
					size="sm"
					variant="outline"
				>
					{locked ? (
						<LockIcon aria-hidden="true" />
					) : (
						<UnlockIcon aria-hidden="true" />
					)}
					{locked ? "Locked" : "Lock"}
					<Kbd className="ms-auto h-4 min-w-4 text-[0.625rem]">C</Kbd>
				</Button>
			</div>
			<div className="flex items-center justify-between gap-3 border-t pt-3 text-sm">
				<span className="flex flex-col">
					<span>Zoom buttons</span>
					<span className="text-muted-foreground text-xs">
						On-screen +/− near the stage edge
					</span>
				</span>
				<ToggleGroup
					aria-label="On-screen zoom buttons"
					onValueChange={(v) => {
						const next = v[0] as ZoomControlMode | undefined;
						if (next) setPrefs({ zoomControls: next });
					}}
					size="sm"
					value={[prefs.zoomControls]}
					variant="outline"
				>
					{(["auto", "always", "never"] as const).map((mode) => (
						<ToggleGroupItem key={mode} value={mode}>
							{ZOOM_MODE_LABEL[mode]}
						</ToggleGroupItem>
					))}
				</ToggleGroup>
			</div>
		</div>
	);
}

// ------------------------------------------------------------- display

function DisplayTab({
	prefs,
	view,
	model,
}: ViewPanelProps): React.ReactElement {
	const rigged = Boolean(model?.rigged);
	const toon = prefs.shading === "toon";
	return (
		<div className="flex flex-col gap-1">
			<div className="flex items-center justify-between gap-3 py-1 text-sm">
				<span className="flex flex-col">
					<ShortcutHint label="Surface" shortcut="U" />
					<span className="text-muted-foreground text-xs">
						{prefs.surface === "shaded"
							? "Materials as shaded"
							: prefs.surface === "albedo"
								? "Base colour, unlit"
								: "UV checker"}
					</span>
				</span>
				<ToggleGroup
					aria-label="Surface"
					onValueChange={(v) => {
						const next = v[0] as SurfaceId | undefined;
						if (next) setPrefs({ surface: next });
					}}
					size="sm"
					value={[prefs.surface]}
					variant="outline"
				>
					<ToggleGroupItem value="shaded">Shaded</ToggleGroupItem>
					<ToggleGroupItem value="albedo">Albedo</ToggleGroupItem>
					<ToggleGroupItem value="uv">UV</ToggleGroupItem>
				</ToggleGroup>
			</div>
			<Row
				checked={prefs.wireframe}
				label="Wireframe"
				onChange={(wireframe) => setPrefs({ wireframe })}
				shortcut="W"
			/>
			<Row
				checked={prefs.wireframe && prefs.wireframeOverlay}
				disabled={!prefs.wireframe}
				hint="Lines drawn over the shaded model"
				label="Wireframe overlay"
				onChange={(wireframeOverlay) => setPrefs({ wireframeOverlay })}
				shortcut="⇧W"
			/>
			<Row
				checked={prefs.outlines && toon}
				disabled={!toon}
				hint={toon ? undefined : "Toon shading only"}
				label="Outlines"
				onChange={(outlines) => setPrefs({ outlines })}
			/>
			<PanelHeading>Helpers</PanelHeading>
			<Row
				checked={prefs.skeleton && rigged}
				disabled={!rigged}
				hint={rigged ? "Bones over the mesh" : "Only rigged models have bones"}
				label="Skeleton"
				onChange={(skeleton) => setPrefs({ skeleton })}
				shortcut="K"
			/>
			<Row
				checked={view.normals}
				hint={
					view.selection
						? "For the selected mesh"
						: "Sampled across the model; pick a mesh for all of its normals"
				}
				label="Vertex normals"
				onChange={(normals) => setViewState({ normals })}
				shortcut="N"
			/>
			<Row
				checked={prefs.bounds}
				label="Bounding box"
				onChange={(bounds) => setPrefs({ bounds })}
				shortcut="X"
			/>
			<Row
				checked={prefs.grid}
				label="Floor grid"
				onChange={(grid) => setPrefs({ grid })}
				shortcut="G"
			/>
			<Row
				checked={prefs.axes}
				hint="X red, Y green, Z blue"
				label="Axes"
				onChange={(axes) => setPrefs({ axes })}
				shortcut="⇧G"
			/>
		</div>
	);
}

// ------------------------------------------------------------- inspect

export function useSelectedMesh(
	model: LoadedModel | null,
	view: ViewState,
): MeshInfo | null {
	return React.useMemo(() => {
		const sel = view.selection;
		if (!model || !sel) return null;
		const mesh = model.content.getObjectByProperty("uuid", sel.meshId);
		if (!mesh || !(mesh as { isMesh?: boolean }).isMesh) return null;
		return describeMesh(
			mesh as Parameters<typeof describeMesh>[0],
			sel.materialIndex,
			model.content,
		);
	}, [model, view.selection]);
}

function InspectTab({
	prefs,
	view,
	model,
	hiddenParts,
	onHiddenChange,
	onFocus,
	onIsolateFocus,
	touch,
}: ViewPanelProps): React.ReactElement {
	const info = useSelectedMesh(model, view);
	const stats = useRenderStats();
	const partId = view.selection?.partId ?? null;
	const isolate = () => {
		if (!model || !partId) return;
		onHiddenChange(
			new Set(model.parts.filter((p) => p.id !== partId).map((p) => p.id)),
		);
	};
	const hide = () => {
		if (!partId) return;
		const next = new Set(hiddenParts);
		next.add(partId);
		onHiddenChange(next);
		setViewState({ selection: null });
	};
	return (
		<div className="flex flex-col gap-1">
			<Row
				checked={view.picking}
				hint={
					touch
						? "Tap a mesh to see what it is"
						: "Click a mesh to see what it is"
				}
				label="Pick mesh"
				onChange={(picking) => setViewState({ picking })}
				shortcut="I"
			/>
			<Row
				checked={prefs.stats}
				hint={
					stats && prefs.stats
						? `${stats.fps < 0.5 ? "idle" : `${stats.fps.toFixed(0)} fps`} · ${formatNumber(stats.calls)} calls`
						: "FPS, draw calls, triangles, textures"
				}
				label="Performance stats"
				onChange={(s) => setPrefs({ stats: s })}
				shortcut="⇧S"
			/>
			<PanelHeading
				action={
					info && (
						<Button
							onClick={() => setViewState({ selection: null })}
							size="xs"
							variant="ghost"
						>
							Clear
						</Button>
					)
				}
			>
				Selection
			</PanelHeading>
			{info ? (
				<div className="flex flex-col gap-2 rounded-lg border bg-background/40 p-2.5">
					<div className="flex min-w-0 flex-col">
						<span className="truncate font-medium text-sm" title={info.name}>
							{info.name}
						</span>
						{info.path.length > 0 && (
							<span className="truncate font-mono text-[0.6875rem] text-muted-foreground">
								{info.path.join(" / ")}
							</span>
						)}
					</div>
					<dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
						<dt className="text-muted-foreground">Shader</dt>
						<dd className="min-w-0">
							{info.shader ? (
								<Badge
									className="max-w-full font-mono"
									size="sm"
									title={info.shader}
									variant="secondary"
								>
									<span className="truncate">{shortShader(info.shader)}</span>
								</Badge>
							) : (
								<span className="text-muted-foreground">Standard PBR</span>
							)}
						</dd>
						<dt className="text-muted-foreground">Material</dt>
						<dd className="truncate font-mono" title={info.materialName}>
							{info.materialName}
							{info.materialCount > 1 &&
								` (${info.materialIndex + 1}/${info.materialCount})`}
						</dd>
						<dt className="text-muted-foreground">Geometry</dt>
						<dd className="tabular-nums">
							{formatNumber(info.triangles)} tris ·{" "}
							{formatNumber(info.vertices)} verts
							{info.skinned && " · skinned"}
						</dd>
						<dt className="text-muted-foreground">Textures</dt>
						<dd className="min-w-0">
							{info.textures.length ? (
								<ul className="flex flex-col gap-0.5">
									{info.textures.map((t) => (
										<li
											className="flex items-baseline justify-between gap-2"
											key={t.slot}
										>
											<span className="truncate font-mono" title={t.slot}>
												{t.slot}
											</span>
											{t.detail && (
												<span
													className="truncate text-muted-foreground"
													title={t.detail}
												>
													{t.detail}
												</span>
											)}
										</li>
									))}
								</ul>
							) : (
								<span className="text-muted-foreground">None</span>
							)}
						</dd>
					</dl>
					<div className="grid grid-cols-2 gap-1.5 pt-0.5">
						<Button
							aria-pressed={view.focus === view.selection?.meshId}
							onClick={onFocus}
							size="xs"
							title="Frame the camera on this mesh (.)"
							variant="outline"
						>
							<CrosshairIcon aria-hidden="true" />
							Focus
						</Button>
						<Button
							disabled={!partId}
							onClick={onIsolateFocus}
							size="xs"
							title="Show only this part and frame it (/)"
							variant="outline"
						>
							<ScanSearchIcon aria-hidden="true" />
							Isolate & focus
						</Button>
						<Button
							disabled={!partId}
							onClick={isolate}
							size="xs"
							variant="outline"
						>
							<ScanIcon aria-hidden="true" />
							Isolate
						</Button>
						<Button
							disabled={!partId}
							onClick={hide}
							size="xs"
							variant="outline"
						>
							<EyeOffIcon aria-hidden="true" />
							Hide
						</Button>
						<Button
							aria-pressed={view.inspector}
							className="col-span-2"
							onClick={() => setViewState({ inspector: !view.inspector })}
							size="xs"
							variant={view.inspector ? "secondary" : "ghost"}
						>
							<InfoIcon aria-hidden="true" />
							{view.inspector
								? "Hide details"
								: "Details: geometry, material, textures"}
						</Button>
						{model && hiddenParts.size > 0 && (
							<Button
								className="col-span-2"
								onClick={() => onHiddenChange(new Set())}
								size="xs"
								variant="ghost"
							>
								Show all parts
							</Button>
						)}
					</div>
				</div>
			) : (
				<p className="flex items-start gap-2 rounded-lg border border-dashed p-2.5 text-muted-foreground text-xs">
					<MousePointerClickIcon
						aria-hidden="true"
						className="mt-0.5 size-3.5 shrink-0"
					/>
					{model
						? view.picking
							? touch
								? "Tap a part of the model to inspect it."
								: "Click a part of the model to inspect it."
							: "Turn on Pick mesh, then tap the model to inspect it."
						: "Load a model to inspect its meshes."}
				</p>
			)}
		</div>
	);
}

// ------------------------------------------------------------- capture

function CaptureTab({ prefs, model, onCapture }: ViewPanelProps) {
	const stats = useRenderStats();
	return (
		<div className="flex flex-col gap-2">
			<Row
				checked={prefs.captureTransparent}
				hint="PNG with alpha, no backdrop"
				label="Transparent background"
				onChange={(captureTransparent) => setPrefs({ captureTransparent })}
			/>
			<Button disabled={!model} onClick={onCapture} size="sm">
				<CameraIcon aria-hidden="true" />
				Save PNG
				<Kbd className="ms-auto h-4 min-w-4 text-[0.625rem]">S</Kbd>
			</Button>
			<p className="text-muted-foreground text-xs">
				{stats
					? `Saves the stage at ${stats.width}×${stats.height}, as shown, without the interface.`
					: "Saves the stage as shown, without the interface."}
			</p>
		</div>
	);
}

// ------------------------------------------------------------- panel

export function ViewPanel(props: ViewPanelProps): React.ReactElement {
	const { prefs } = props;
	return (
		<Tabs
			className="w-full gap-3 sm:w-80"
			onValueChange={(v) => setPrefs({ viewTab: v as ViewTab })}
			value={prefs.viewTab}
		>
			<TabsList className="w-full" size="sm">
				<TabsTab value="camera">Camera</TabsTab>
				<TabsTab value="display">Display</TabsTab>
				<TabsTab value="inspect">Inspect</TabsTab>
				<TabsTab value="capture">Capture</TabsTab>
			</TabsList>
			<TabsPanel value="camera">
				<CameraTab {...props} />
			</TabsPanel>
			<TabsPanel value="display">
				<DisplayTab {...props} />
			</TabsPanel>
			<TabsPanel value="inspect">
				<InspectTab {...props} />
			</TabsPanel>
			<TabsPanel value="capture">
				<CaptureTab {...props} />
			</TabsPanel>
		</Tabs>
	);
}
