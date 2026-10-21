/** Loading, empty, error, welcome, help and stats overlays. */
import {
	CrosshairIcon,
	EyeOffIcon,
	InfoIcon,
	MonitorOffIcon,
	MousePointer2Icon,
	ScanSearchIcon,
	SparklesIcon,
	TriangleAlertIcon,
	XIcon,
} from "lucide-react";
import type * as React from "react";
import type { MeshInfo } from "@/components/studio/inspect";
import { shortShader } from "@/components/studio/inspect";
import type { LoadedModel } from "@/components/studio/load-model";
import { useRenderStats } from "@/components/studio/stats-store";
import { formatMB, GlassCard, IconButton } from "@/components/studio/ui/chrome";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogDescription,
	DialogHeader,
	DialogPanel,
	DialogPopup,
	DialogTitle,
} from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

export function LoadingCard({
	name,
	loaded,
	total,
}: {
	name: string;
	loaded: number;
	total: number;
}): React.ReactElement {
	const pct = total > 0 ? Math.min(100, (loaded / total) * 100) : 0;
	return (
		<>
			<div
				aria-hidden="true"
				className="pointer-events-none fixed inset-x-0 top-0 z-40 h-0.5 bg-primary/15"
			>
				<div
					className="h-full bg-primary transition-[width] duration-200 ease-out"
					style={{ width: `${Math.max(pct, 3)}%` }}
				/>
			</div>
			<div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center p-6">
				<GlassCard
					aria-live="polite"
					className="flex w-64 flex-col items-center gap-3 px-5 py-4 text-center"
					role="status"
				>
					<Spinner className="size-5 text-primary" />
					<div className="flex min-w-0 flex-col gap-0.5">
						<span className="truncate font-medium text-sm">
							Loading {name}…
						</span>
						<span className="font-mono text-muted-foreground text-xs tabular-nums">
							{total > 0
								? `${formatMB(loaded)} / ${formatMB(total)} MB`
								: "Starting…"}
						</span>
					</div>
					<div className="h-1 w-full overflow-hidden rounded-full bg-muted">
						<div
							className="h-full rounded-full bg-primary transition-[width] duration-200 ease-out"
							style={{ width: `${pct}%` }}
						/>
					</div>
				</GlassCard>
			</div>
		</>
	);
}

export function EmptyStage({
	onBrowse,
}: {
	onBrowse: () => void;
}): React.ReactElement {
	return (
		<div className="pointer-events-none absolute inset-x-0 top-[18dvh] z-10 flex justify-center p-6">
			<GlassCard className="pointer-events-auto flex max-w-xs flex-col items-center gap-3 px-6 py-5 text-center">
				<div className="flex size-11 items-center justify-center rounded-xl bg-primary/12">
					<SparklesIcon aria-hidden="true" className="size-5 text-primary" />
				</div>
				<div className="flex flex-col gap-1">
					<h2 className="font-semibold text-base text-foreground">
						Pick something to look at
					</h2>
					<p className="text-muted-foreground text-sm">
						Operators, enemies, weapons and NPCs, straight from the game files.
					</p>
				</div>
				<Button onClick={onBrowse} size="sm">
					Browse models
				</Button>
			</GlassCard>
		</div>
	);
}

export function ErrorStage({
	title,
	message,
	onRetry,
	onBrowse,
}: {
	title: string;
	message: string;
	onRetry?: () => void;
	onBrowse: () => void;
}): React.ReactElement {
	return (
		<div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center p-6">
			<GlassCard className="pointer-events-auto flex max-w-sm flex-col items-center gap-3 px-6 py-5 text-center">
				<TriangleAlertIcon
					aria-hidden="true"
					className="size-5 text-destructive-foreground"
				/>
				<div className="flex flex-col gap-1">
					<h2 className="font-semibold text-sm">{title}</h2>
					<p className="break-words text-muted-foreground text-xs">{message}</p>
				</div>
				<div className="flex gap-2">
					{onRetry && (
						<Button onClick={onRetry} size="sm" variant="outline">
							Try again
						</Button>
					)}
					<Button onClick={onBrowse} size="sm">
						Pick another
					</Button>
				</div>
			</GlassCard>
		</div>
	);
}

export function ContextLostCard(): React.ReactElement {
	return (
		<div className="absolute inset-0 z-30 flex items-center justify-center bg-background/40 p-6 backdrop-blur-sm">
			<GlassCard
				className="flex max-w-sm flex-col items-center gap-3 px-6 py-5 text-center"
				role="alert"
			>
				<MonitorOffIcon
					aria-hidden="true"
					className="size-5 text-warning-foreground"
				/>
				<div className="flex flex-col gap-1">
					<h2 className="font-semibold text-sm">
						The graphics context was lost
					</h2>
					<p className="text-muted-foreground text-xs">
						The GPU was reset or ran out of memory. The Studio picks up again as
						soon as the browser restores it.
					</p>
				</div>
				<Button
					onClick={() => window.location.reload()}
					size="sm"
					variant="outline"
				>
					Reload now
				</Button>
			</GlassCard>
		</div>
	);
}

export function WelcomeHint({
	touch,
	onDismiss,
}: {
	touch: boolean;
	onDismiss: () => void;
}): React.ReactElement {
	return (
		<GlassCard
			aria-label="Getting started"
			className="pointer-events-auto relative flex w-[min(22rem,calc(100vw-2rem))] gap-3 p-3 pe-9"
			role="region"
		>
			<div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/12 text-primary">
				<MousePointer2Icon aria-hidden="true" className="size-4" />
			</div>
			<div className="flex min-w-0 flex-col gap-1 text-sm">
				<span className="font-medium">Welcome to the Studio</span>
				<span className="text-muted-foreground text-xs leading-relaxed">
					{touch
						? "Drag to orbit, pinch to zoom and use two fingers to pan."
						: "Drag to orbit, right-drag or Shift-drag to pan, scroll to zoom."}{" "}
					Models are shown in their rest pose with approximate materials.
					{!touch && (
						<>
							{" "}
							Press <Kbd className="h-4 min-w-4 text-[0.625rem]">?</Kbd> for
							every shortcut.
						</>
					)}
				</span>
				<Button className="mt-1 self-start" onClick={onDismiss} size="xs">
					Got it
				</Button>
			</div>
			<Button
				aria-label="Dismiss"
				className="absolute end-1.5 top-1.5"
				onClick={onDismiss}
				size="icon-xs"
				variant="ghost"
			>
				<XIcon />
			</Button>
		</GlassCard>
	);
}

interface Shortcut {
	keys: string[];
	label: string;
	clips?: true;
}

const SHORTCUT_GROUPS: { title: string; items: Shortcut[] }[] = [
	{
		title: "Panels",
		items: [
			{ keys: ["M"], label: "Model picker" },
			{ keys: ["P"], label: "Parts" },
			{ keys: ["A"], label: "Animations", clips: true },
			{ keys: ["B"], label: "Backdrop" },
			{ keys: ["L"], label: "Lighting" },
			{ keys: ["V"], label: "View tools" },
			{ keys: ["H"], label: "Hide or show the interface" },
			{ keys: ["Esc"], label: "Close the panel, clear the selection" },
			{ keys: ["?"], label: "This help" },
		],
	},
	{
		title: "Camera",
		items: [
			{ keys: ["F"], label: "Reset the view and the shown parts" },
			{ keys: ["."], label: "Focus on the selected mesh" },
			{ keys: ["/"], label: "Isolate and focus on the selected part" },
			{ keys: ["1", "…", "6"], label: "Front, back, right, left, top, ¾" },
			{ keys: ["=", "−"], label: "Zoom in or out" },
			{ keys: ["Shift", "←↑↓→"], label: "Pan the view" },
			{ keys: ["C"], label: "Lock or unlock the camera" },
			{ keys: ["O"], label: "Auto-rotate" },
		],
	},
	{
		title: "Display",
		items: [
			{ keys: ["T"], label: "Toon or standard shading" },
			{ keys: ["U"], label: "Shaded, albedo or UV checker" },
			{ keys: ["W"], label: "Wireframe" },
			{ keys: ["Shift", "W"], label: "Wireframe overlay" },
			{ keys: ["K"], label: "Skeleton" },
			{ keys: ["N"], label: "Vertex normals" },
			{ keys: ["X"], label: "Bounding box" },
			{ keys: ["G"], label: "Floor grid" },
			{ keys: ["Shift", "G"], label: "Axes" },
		],
	},
	{
		title: "Inspect and capture",
		items: [
			{ keys: ["I"], label: "Pick meshes by clicking" },
			{ keys: ["⌫"], label: "Hide the selected part" },
			{ keys: ["Shift", "S"], label: "Performance stats" },
			{ keys: ["S"], label: "Save a screenshot" },
		],
	},
	{
		title: "Playback",
		items: [
			{ keys: ["Space"], label: "Play or pause", clips: true },
			{ keys: ["←", "→"], label: "Step one frame", clips: true },
			{ keys: ["R"], label: "Restart the animation", clips: true },
		],
	},
];

const GESTURES: { input: string; mouse: string; touch: string }[] = [
	{ input: "Orbit", mouse: "Left-drag", touch: "One finger" },
	{
		input: "Pan",
		mouse: "Right-drag, or Shift / Ctrl + drag",
		touch: "Two fingers, or one with pan mode",
	},
	{
		input: "Zoom",
		mouse: "Scroll or pinch the trackpad, towards the pointer",
		touch: "Pinch, or +/−",
	},
	{
		input: "Inspect",
		mouse: "Click a mesh with Pick on",
		touch: "Tap a mesh with Pick on",
	},
	{
		input: "Focus",
		mouse: "Double-click a mesh",
		touch: "Double-tap a mesh",
	},
];

export function HelpDialog({
	open,
	onOpenChange,
	hasClips,
}: {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	/** Playback shortcuts only apply to models with animation clips. */
	hasClips: boolean;
}): React.ReactElement {
	return (
		<Dialog onOpenChange={onOpenChange} open={open}>
			<DialogPopup className="max-w-2xl">
				<DialogHeader>
					<DialogTitle>Studio controls</DialogTitle>
					<DialogDescription>
						Look around a model with the mouse or touch, and drive everything
						else from the keyboard or the View tools.
					</DialogDescription>
				</DialogHeader>
				<DialogPanel className="grid max-h-[70dvh] gap-6 overflow-y-auto sm:grid-cols-[1fr_1.3fr]">
					<section className="flex flex-col gap-2">
						<h3 className="font-medium text-muted-foreground text-xs uppercase tracking-wide">
							Gestures
						</h3>
						<dl className="flex flex-col gap-2 text-sm">
							{GESTURES.map((g) => (
								<div className="flex flex-col" key={g.input}>
									<dt className="font-medium">{g.input}</dt>
									<dd className="text-muted-foreground text-xs">
										{g.mouse} · {g.touch}
									</dd>
								</div>
							))}
						</dl>
					</section>
					<div className="flex flex-col gap-4">
						{SHORTCUT_GROUPS.map((group) => {
							const items = group.items.filter((s) => hasClips || !s.clips);
							if (!items.length) return null;
							return (
								<section className="flex flex-col gap-1.5" key={group.title}>
									<h3 className="font-medium text-muted-foreground text-xs uppercase tracking-wide">
										{group.title}
									</h3>
									<dl className="flex flex-col gap-1 text-sm">
										{items.map((s) => (
											<div
												className="flex items-center justify-between gap-3"
												key={s.label}
											>
												<dt className="text-muted-foreground">{s.label}</dt>
												<dd className="flex shrink-0 gap-1">
													{s.keys.map((k) => (
														<Kbd key={k}>{k}</Kbd>
													))}
												</dd>
											</div>
										))}
									</dl>
								</section>
							);
						})}
					</div>
				</DialogPanel>
			</DialogPopup>
		</Dialog>
	);
}

export function StatsCard({
	model,
	className,
}: {
	model: LoadedModel | null;
	className?: string;
}): React.ReactElement {
	const stats = useRenderStats();
	const rows: [string, string][] = [
		["FPS", stats ? (stats.fps < 0.5 ? "idle" : stats.fps.toFixed(0)) : "–"],
		["Draw calls", stats ? formatNumber(stats.calls) : "–"],
		["Triangles", stats ? formatNumber(stats.triangles) : "–"],
		["Model tris", model ? formatNumber(model.stats.triangles) : "–"],
		["Meshes", model ? formatNumber(model.stats.meshes) : "–"],
		[
			"Textures",
			stats
				? `${model?.stats.textures ?? 0} model · ${stats.textures} GPU`
				: "–",
		],
		["Download", model ? `${formatMB(model.stats.bytes)} MB` : "–"],
		["Pixel ratio", stats ? stats.dpr.toFixed(2) : "–"],
		["Buffer", stats ? `${stats.width}×${stats.height}` : "–"],
	];
	return (
		<GlassCard className={cn("w-48 px-3 py-2", className)}>
			<dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 font-mono text-[0.6875rem]">
				{rows.map(([k, v]) => (
					<div className="contents" key={k}>
						<dt className="text-muted-foreground">{k}</dt>
						<dd className="truncate text-end tabular-nums">{v}</dd>
					</div>
				))}
			</dl>
		</GlassCard>
	);
}

/**
 * Floating chip for the picked mesh: what it is and the game shader that
 * draws it, with focus / isolate / hide at hand even when the View panel is
 * closed, and a toggle for the detail inspector below it.
 */
export function SelectionCard({
	info,
	canIsolate,
	focused,
	detailsOpen,
	onFocus,
	onIsolateFocus,
	onHide,
	onDetails,
	onClear,
	className,
}: {
	info: MeshInfo;
	canIsolate: boolean;
	/** The camera is framed on this mesh. */
	focused: boolean;
	detailsOpen: boolean;
	onFocus: () => void;
	onIsolateFocus: () => void;
	onHide: () => void;
	onDetails: () => void;
	onClear: () => void;
	className?: string;
}): React.ReactElement {
	return (
		<GlassCard
			aria-label="Selected mesh"
			className={cn("flex items-center gap-2 p-1.5 ps-3", className)}
			role="region"
		>
			<span
				aria-hidden="true"
				className="size-2 shrink-0 rounded-full bg-[#8b7cf6] shadow-[0_0_0_3px_rgb(139_124_246/0.25)]"
			/>
			<div className="flex min-w-0 flex-col">
				<span className="truncate font-medium text-sm leading-tight">
					{info.name}
				</span>
				<span className="flex min-w-0 items-center gap-1.5 overflow-hidden text-muted-foreground text-xs leading-tight">
					{info.shader ? (
						<Badge
							className="min-w-0 shrink font-mono text-[0.625rem]"
							size="sm"
							title={info.shader}
							variant="secondary"
						>
							<span className="truncate">{shortShader(info.shader)}</span>
						</Badge>
					) : (
						<span className="truncate">{info.materialName}</span>
					)}
					<span className="shrink-0 tabular-nums">
						{formatNumber(info.triangles)} tris
					</span>
				</span>
			</div>
			<div className="ms-1 flex shrink-0 items-center gap-0.5 border-s ps-1">
				<IconButton
					aria-pressed={focused}
					className={cn(focused && "bg-primary/12 text-primary")}
					label={focused ? "Focused on this mesh" : "Focus on this mesh"}
					onClick={onFocus}
					shortcut="."
				>
					<CrosshairIcon />
				</IconButton>
				<IconButton
					disabled={!canIsolate}
					label="Isolate and focus"
					onClick={onIsolateFocus}
					shortcut="/"
				>
					<ScanSearchIcon />
				</IconButton>
				<IconButton
					disabled={!canIsolate}
					label="Hide this part"
					onClick={onHide}
					shortcut="⌫"
				>
					<EyeOffIcon />
				</IconButton>
				<IconButton
					aria-expanded={detailsOpen}
					aria-pressed={detailsOpen}
					className={cn(detailsOpen && "bg-primary/12 text-primary")}
					label={
						detailsOpen
							? "Hide details"
							: "Details: geometry, material, textures"
					}
					onClick={onDetails}
				>
					<InfoIcon />
				</IconButton>
				<IconButton label="Clear selection" onClick={onClear} shortcut="Esc">
					<XIcon />
				</IconButton>
			</div>
		</GlassCard>
	);
}
