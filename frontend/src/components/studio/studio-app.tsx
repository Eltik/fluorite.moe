/**
 * The Studio: a full-screen 3D model viewer. This module is the lazy chunk
 * root, so three / r3f / drei only load when the Studio opens.
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
	Link,
	useNavigate,
	useParams,
	useSearch,
} from "@tanstack/react-router";
import {
	ArrowLeftIcon,
	BoxIcon,
	CameraIcon,
	CircleHelpIcon,
	ClapperboardIcon,
	EyeIcon,
	EyeOffIcon,
	FocusIcon,
	LayersIcon,
	LockIcon,
	MoonIcon,
	PaletteIcon,
	ScanEyeIcon,
	SettingsIcon,
	SunIcon,
	SunMediumIcon,
	UnlockIcon,
} from "lucide-react";
import * as React from "react";
import { LogoMark } from "@/components/shell/logo";
import { AnimationController } from "@/components/studio/animation";
import { isAbortError } from "@/components/studio/bytes";
import {
	type StudioModelSource,
	studioCatalogQuery,
	studioDetailQuery,
	UnknownModelError,
} from "@/components/studio/catalog-source";
import { describeMeshDetail } from "@/components/studio/inspect";
import { type LoadedModel, loadModel } from "@/components/studio/load-model";
import {
	type BackgroundId,
	getPrefs,
	isHexColor,
	setPrefs,
	usePrefs,
} from "@/components/studio/prefs";
import { BACKGROUNDS, resolveBackground } from "@/components/studio/presets";
import { Stage, type StageHandle } from "@/components/studio/stage";
import {
	AnimationsBrowser,
	AnimationsCard,
} from "@/components/studio/ui/animations-panel";
import { GlassCard, IconButton } from "@/components/studio/ui/chrome";
import { Flyout } from "@/components/studio/ui/flyout";
import { InspectorCard } from "@/components/studio/ui/inspector";
import { ModelPicker } from "@/components/studio/ui/model-picker";
import {
	ContextLostCard,
	EmptyStage,
	ErrorStage,
	HelpDialog,
	LoadingCard,
	SelectionCard,
	StatsCard,
	WelcomeHint,
} from "@/components/studio/ui/overlays";
import {
	BackgroundPanel,
	LightingPanel,
	PartsPanel,
} from "@/components/studio/ui/panels";
import { SettingsPanel } from "@/components/studio/ui/settings-panel";
import { ToolbarButton } from "@/components/studio/ui/toolbar-button";
import { TransportBar } from "@/components/studio/ui/transport";
import { useSelectedMesh, ViewPanel } from "@/components/studio/ui/view-panel";
import { ZoomControls } from "@/components/studio/ui/zoom-controls";
import { useStudioShortcuts } from "@/components/studio/use-shortcuts";
import {
	resetViewState,
	type Selection,
	setViewState,
	useViewState,
} from "@/components/studio/view-state";
import { Badge } from "@/components/ui/badge";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import { toastManager } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useMediaQuery } from "@/hooks/use-media-query";
import { clipLabel, clipStem, defaultClip } from "@/lib/clips";
import { pageTitle } from "@/lib/route-meta";
import { updateSettings, useSettings } from "@/lib/settings";
import {
	isStudioKind,
	STUDIO_KIND_SINGULAR,
	type StudioKind,
	type StudioSearch,
} from "@/lib/studio";
import { cn } from "@/lib/utils";

export type FlyoutId =
	| "models"
	| "parts"
	| "animations"
	| "background"
	| "lighting"
	| "view"
	| "settings";

type LoadState =
	| { status: "idle" }
	| { status: "loading"; name: string; loaded: number; total: number }
	| { status: "ready" }
	| { status: "error"; name: string; message: string };

function hasWebGL2(): boolean {
	try {
		return Boolean(document.createElement("canvas").getContext("webgl2"));
	} catch {
		return false;
	}
}

/** Tracks the `dark` class on <html>, which also follows system changes. */
function useHtmlDark(): boolean {
	return React.useSyncExternalStore(
		(cb) => {
			const mo = new MutationObserver(cb);
			mo.observe(document.documentElement, {
				attributes: true,
				attributeFilter: ["class"],
			});
			return () => mo.disconnect();
		},
		() => document.documentElement.classList.contains("dark"),
		() => false,
	);
}

function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

/** The parts a model hides out of the box (LOD levels, proxies, VFX). */
function defaultHidden(model: LoadedModel): Set<string> {
	return new Set(model.parts.filter((p) => p.hiddenByDefault).map((p) => p.id));
}

export default function StudioApp(): React.ReactElement {
	const [supported] = React.useState(hasWebGL2);
	if (!supported) return <Unsupported />;
	return (
		<TooltipProvider delay={400}>
			<Studio />
		</TooltipProvider>
	);
}

function Unsupported(): React.ReactElement {
	return (
		<main className="flex h-dvh w-screen items-center justify-center bg-background p-6">
			<div className="flex max-w-sm flex-col items-center gap-3 text-center">
				<BoxIcon aria-hidden="true" className="size-6 text-muted-foreground" />
				<h1 className="font-semibold text-lg">WebGL 2 isn't available</h1>
				<p className="text-muted-foreground text-sm">
					The Studio renders models with WebGL 2, which this browser or device
					has turned off. Try another browser, or enable hardware acceleration.
				</p>
				<Link
					className="text-primary text-sm underline-offset-4 hover:underline"
					to="/"
				>
					Back to fluorite.moe
				</Link>
			</div>
		</main>
	);
}

function Studio(): React.ReactElement {
	const params = useParams({ strict: false }) as { kind?: string; id?: string };
	const search = useSearch({ strict: false }) as StudioSearch;
	const navigate = useNavigate();
	const queryClient = useQueryClient();
	const { textLanguage: lang } = useSettings();
	const themeDark = useHtmlDark();
	const prefs = usePrefs();
	const view = useViewState();
	const mobile = useMediaQuery("max-sm");
	const touch = useMediaQuery("(pointer: coarse)");

	const kind = isStudioKind(params.kind) ? params.kind : undefined;
	const id = kind ? params.id : undefined;

	const [flyout, setFlyout] = React.useState<FlyoutId | null>(null);
	// `/studio` opens with the picker. Deferred past hydration so the popover's
	// outside-inert markers never touch server-rendered markup.
	// biome-ignore lint/correctness/useExhaustiveDependencies: on mount only
	React.useEffect(() => {
		if (id) return;
		const t = window.setTimeout(() => setFlyout("models"), 150);
		return () => window.clearTimeout(t);
	}, []);
	const pickerTab: StudioKind = prefs.pickerKind;
	const setPickerTab = React.useCallback((k: StudioKind) => {
		setPrefs({ pickerKind: k });
	}, []);
	const [uiHidden, setUiHidden] = React.useState(false);
	const [locked, setLocked] = React.useState(false);
	const [helpOpen, setHelpOpen] = React.useState(false);
	const [contextLost, setContextLost] = React.useState(false);
	const [load, setLoad] = React.useState<LoadState>({ status: "idle" });
	const [model, setModel] = React.useState<LoadedModel | null>(null);
	const [shown, setShown] = React.useState<LoadedModel | null>(null);
	const [hiddenParts, setHiddenParts] = React.useState<Set<string>>(
		() => new Set(),
	);
	const [retry, setRetry] = React.useState(0);

	const stageRef = React.useRef<StageHandle | null>(null);
	const toolbarRef = React.useRef<HTMLDivElement>(null);
	const pickerSearchRef = React.useRef<HTMLInputElement>(null);
	const clipSearchRef = React.useRef<HTMLInputElement>(null);
	const animationsButtonRef = React.useRef<HTMLButtonElement>(null);
	const models = React.useRef(new Set<LoadedModel>());
	const modelRef = React.useRef<LoadedModel | null>(null);
	modelRef.current = model;

	// Unknown kind in the URL: back to the picker.
	React.useEffect(() => {
		if (params.kind && !kind) {
			toastManager.add({
				type: "error",
				title: "Unknown model type",
				description: `“${params.kind}” isn't something the Studio can show.`,
			});
			void navigate({ to: "/studio", replace: true });
		}
	}, [params.kind, kind, navigate]);

	React.useEffect(() => {
		if (kind) setPickerTab(kind);
	}, [kind, setPickerTab]);

	// ---------------------------------------------------------- data

	const detail = useQuery({
		...studioDetailQuery(kind ?? "operators", id ?? "", lang, () =>
			queryClient.ensureQueryData(
				studioCatalogQuery(kind ?? "operators", lang),
			),
		),
		enabled: Boolean(kind && id),
	});
	const catalogSource: StudioModelSource | undefined =
		kind && id ? detail.data : undefined;
	// Dev builds can point the Studio at any rigged GLB (plus a volume JSON)
	// to test the shaders against other exporters' files.
	const devGlb = import.meta.env.DEV ? search.glb : undefined;
	const devVolume = import.meta.env.DEV ? search.volume : undefined;
	const devClips = import.meta.env.DEV ? search.clips : undefined;
	const source = React.useMemo<StudioModelSource | undefined>(() => {
		if (!devGlb) return catalogSource;
		return {
			entry: catalogSource?.entry ?? {
				kind: kind ?? "operators",
				id: id ?? "dev",
				name: id ?? "Dev GLB",
				tags: [],
			},
			rigged: devGlb,
			riggedUrl: devGlb,
			volumeUrl: devVolume,
			clipsUrl: devClips,
			parts: [],
			source: "api",
		};
	}, [catalogSource, devGlb, devVolume, devClips, kind, id]);

	React.useEffect(() => {
		if (!detail.error || !kind || !id) return;
		if (detail.error instanceof UnknownModelError) {
			toastManager.add({
				type: "error",
				title: "Model not found",
				description: `There's no ${STUDIO_KIND_SINGULAR[kind].toLowerCase()} with the id “${id}”.`,
			});
		} else {
			toastManager.add({
				type: "error",
				title: "Couldn't load model data",
				description: errorMessage(detail.error),
			});
		}
		setFlyout("models");
	}, [detail.error, kind, id]);

	// ---------------------------------------------------------- loading

	React.useEffect(() => {
		if (!kind || !id) {
			setModel(null);
			setLoad({ status: "idle" });
		}
	}, [kind, id]);

	// biome-ignore lint/correctness/useExhaustiveDependencies: `retry` re-runs the load
	React.useEffect(() => {
		if (!source) return;
		const controller = new AbortController();
		const name = source.entry.name;
		setLoad({ status: "loading", name, loaded: 0, total: 0 });
		loadModel(source, {
			signal: controller.signal,
			onProgress: (loaded, total) =>
				setLoad((s) => (s.status === "loading" ? { ...s, loaded, total } : s)),
		}).then(
			(loaded) => {
				if (controller.signal.aborted) {
					loaded.dispose();
					return;
				}
				models.current.add(loaded);
				setHiddenParts(defaultHidden(loaded));
				// The picked mesh belonged to the previous model.
				setViewState({ selection: null, focus: null });
				setModel(loaded);
				setLoad({ status: "ready" });
				if (loaded.warnings.length) {
					toastManager.add({
						type: "warning",
						title: `${name} loaded with gaps`,
						description:
							loaded.warnings.length === 1
								? loaded.warnings[0]
								: `${loaded.warnings.length} files couldn't be loaded.`,
					});
				}
			},
			(error: unknown) => {
				if (controller.signal.aborted || isAbortError(error)) return;
				const message = errorMessage(error);
				setLoad({ status: "error", name, message });
				toastManager.add({
					type: "error",
					title: `Couldn't load ${name}`,
					description: message,
				});
			},
		);
		return () => {
			controller.abort();
			// Don't leave this model's card up if nothing replaces it (e.g. the
			// next model's detail query is still pending or fails).
			setLoad((s) =>
				s.status === "loading" && s.name === name ? { status: "idle" } : s,
			);
		};
	}, [source, retry]);

	// Dispose models that are no longer on screen or pending.
	const handleShown = React.useCallback((m: LoadedModel | null) => {
		setShown(m);
		for (const x of models.current) {
			if (x !== m && x !== modelRef.current) {
				x.dispose();
				models.current.delete(x);
			}
		}
	}, []);

	React.useEffect(() => {
		const all = models.current;
		return () => {
			for (const x of all) x.dispose();
			all.clear();
			resetViewState();
		};
	}, []);

	// ---------------------------------------------------------- inspect

	const selected = useSelectedMesh(shown, view);
	const selectedPart = view.selection?.partId ?? null;
	const selectedMeshId = view.selection?.meshId ?? null;
	// The detail inspector's data, built only while it is open.
	const meshDetail = React.useMemo(() => {
		if (!shown || !view.inspector || !view.selection) return null;
		const mesh = shown.content.getObjectByProperty(
			"uuid",
			view.selection.meshId,
		);
		if (!mesh || !(mesh as { isMesh?: boolean }).isMesh) return null;
		return describeMeshDetail(
			mesh as Parameters<typeof describeMeshDetail>[0],
			view.selection.materialIndex,
		);
	}, [shown, view.inspector, view.selection]);
	const handlePick = React.useCallback((selection: Selection | null) => {
		setViewState({ selection });
	}, []);
	const focusSelection = React.useCallback((selection: Selection) => {
		setViewState({ selection });
		if (stageRef.current?.focusOn(selection.meshId)) {
			setViewState({ focus: selection.meshId });
		}
	}, []);
	const focusSelected = React.useCallback(() => {
		if (view.selection) focusSelection(view.selection);
	}, [view.selection, focusSelection]);
	const hideSelected = React.useCallback(() => {
		if (!selectedPart) return;
		setHiddenParts((prev) => {
			const next = new Set(prev);
			next.add(selectedPart);
			return next;
		});
		setViewState({ selection: null });
	}, [selectedPart]);
	/** Solo the selected part and frame it. */
	const isolateFocusSelected = React.useCallback(() => {
		if (!shown || !view.selection) return;
		const part = view.selection.partId;
		if (part) {
			setHiddenParts(
				new Set(shown.parts.filter((p) => p.id !== part).map((p) => p.id)),
			);
		}
		focusSelection(view.selection);
	}, [shown, view.selection, focusSelection]);
	// What the Weapon / Props toggles were when this Studio opened, so Reset
	// can put them back without touching the viewer's saved preference.
	const sessionProps = React.useRef({
		showWeapon: prefs.showWeapon,
		showProps: prefs.showProps,
	});
	/**
	 * Reset: the default camera, the model's default parts, no selection or
	 * focus, and the session's weapon / props toggles. Display overlays
	 * (wireframe, skeleton, normals) are preferences and stay as they are.
	 */
	const resetAll = React.useCallback(() => {
		stageRef.current?.resetView();
		if (shown) setHiddenParts(defaultHidden(shown));
		setViewState({ selection: null, focus: null });
		const { showWeapon, showProps } = sessionProps.current;
		const current = getPrefs();
		if (current.showWeapon !== showWeapon || current.showProps !== showProps) {
			setPrefs({ showWeapon, showProps });
		}
	}, [shown]);
	const showZoomControls =
		prefs.zoomControls === "always" || (prefs.zoomControls === "auto" && touch);

	// ---------------------------------------------------------- animation

	// biome-ignore lint/correctness/useExhaustiveDependencies: root motion is applied below, not rebuilt
	const animation = React.useMemo(
		() =>
			shown?.clips?.entries.length
				? new AnimationController(shown.content, shown.clips, {
						rootMotion: prefs.rootMotion,
						menu: shown.charscreen?.menu ?? null,
					})
				: null,
		[shown],
	);
	React.useEffect(() => () => animation?.dispose(), [animation]);
	React.useEffect(() => {
		animation?.setRootMotion(prefs.rootMotion);
	}, [animation, prefs.rootMotion]);

	const reportClipFailure = React.useCallback(
		(stem: string) => {
			const entry = animation?.entry(stem);
			toastManager.add({
				type: "error",
				title: "Couldn't load the animation",
				description: entry ? clipLabel(entry) : stem,
			});
		},
		[animation],
	);

	// `?clip=` deep link once the clips are known, else the character
	// screen's opening moment (overview start, then its loop), else a
	// sensible idle. Only on a new controller: later URL changes come from
	// selecting clips.
	// biome-ignore lint/correctness/useExhaustiveDependencies: see above
	React.useEffect(() => {
		if (!animation) return;
		const wanted = search.clip;
		if (wanted && animation.entry(wanted)) {
			void animation.select(wanted).then((ok) => {
				if (!ok && animation.state.failed === wanted) reportClipFailure(wanted);
			});
			return;
		}
		void animation.playDefaultMoment().then((ok) => {
			if (ok) return;
			const fallback = defaultClip(animation.entries);
			const stem = fallback ? clipStem(fallback.file) : null;
			if (!stem) return;
			void animation.select(stem).then((done) => {
				if (!done && animation.state.failed === stem) reportClipFailure(stem);
			});
		});
	}, [animation]);

	const selectClip = React.useCallback(
		(stem: string) => {
			if (!animation) return;
			void animation.select(stem).then((ok) => {
				if (!ok && animation.state.failed === stem) reportClipFailure(stem);
			});
			void navigate({
				to: ".",
				search: (prev: StudioSearch) => ({ ...prev, clip: stem }),
				replace: true,
			} as never);
		},
		[animation, navigate, reportClipFailure],
	);

	const selectMoment = React.useCallback(
		(id: string) => {
			if (!animation) return;
			void animation.selectMoment(id).then((ok) => {
				const failed = animation.state.failed;
				if (!ok && failed) reportClipFailure(failed);
			});
			// Moments are the default view; a deep link only names raw clips.
			if (search.clip !== undefined) {
				void navigate({
					to: ".",
					search: (prev: StudioSearch) => ({ ...prev, clip: undefined }),
					replace: true,
				} as never);
			}
		},
		[animation, navigate, reportClipFailure, search.clip],
	);

	const handlePropWarnings = React.useCallback((warnings: string[]) => {
		toastManager.add({
			type: "warning",
			title: "Some props couldn't be loaded",
			description: warnings[0],
		});
	}, []);

	// ---------------------------------------------------------- view

	const background = resolveBackground(
		search.bg,
		prefs.background,
		prefs.customColor,
		themeDark,
	);
	const backgroundChoice: BackgroundId =
		search.bg && isHexColor(search.bg)
			? "custom"
			: search.bg && search.bg in BACKGROUNDS
				? (search.bg as BackgroundId)
				: prefs.background;

	const selectBackground = (bg: BackgroundId, color?: string) => {
		setPrefs(
			color ? { background: bg, customColor: color } : { background: bg },
		);
		if (search.bg !== undefined) {
			void navigate({
				to: ".",
				search: (prev: StudioSearch) => ({
					...prev,
					bg: bg === "custom" ? (color ?? prefs.customColor) : bg,
				}),
				replace: true,
			} as never);
		}
	};

	const name = shown?.name ?? source?.entry.name;
	React.useEffect(() => {
		document.title = pageTitle(...(name ? [name, "Studio"] : ["Studio"]));
	}, [name]);

	const toggleFlyout = React.useCallback((f: FlyoutId) => {
		setFlyout((cur) => (cur === f ? null : f));
	}, []);
	// The desktop animations card is not a popover, so it manages its own
	// open focus (search) and close focus (its toolbar button).
	const animationsOpen = flyout === "animations" && !uiHidden;
	const closeAnimations = React.useCallback(() => {
		setFlyout((cur) => (cur === "animations" ? null : cur));
		animationsButtonRef.current?.focus();
	}, []);
	React.useEffect(() => {
		if (!animationsOpen || mobile) return;
		const t = window.setTimeout(() => clipSearchRef.current?.focus(), 30);
		return () => window.clearTimeout(t);
	}, [animationsOpen, mobile]);
	React.useEffect(() => {
		if (!animation) setFlyout((cur) => (cur === "animations" ? null : cur));
	}, [animation]);
	const flyoutProps = (f: FlyoutId) => ({
		open: flyout === f && !uiHidden,
		onOpenChange: (open: boolean) =>
			setFlyout((cur) => (open ? f : cur === f ? null : cur)),
		mobile,
	});

	const screenshot = React.useCallback(async () => {
		try {
			const base = shown ? `${shown.key.replace("/", "-")}` : "studio";
			await stageRef.current?.screenshot(
				`fluorite-${base}.png`,
				prefs.captureTransparent,
			);
		} catch (error) {
			toastManager.add({
				type: "error",
				title: "Screenshot failed",
				description: errorMessage(error),
			});
		}
	}, [shown, prefs.captureTransparent]);

	useStudioShortcuts({
		enabled: !helpOpen,
		onHelp: () => setHelpOpen((o) => !o),
		onToggleUi: () => {
			setUiHidden((h) => !h);
			setFlyout(null);
		},
		onShowUi: () => setUiHidden(false),
		onEscape: () => {
			if (flyout === "animations") closeAnimations();
			else if (view.selection) setViewState({ selection: null });
		},
		uiHidden,
		onReset: resetAll,
		onPan: (dx, dy) => stageRef.current?.pan(dx, dy),
		onZoom: (factor) => stageRef.current?.zoomBy(factor),
		onView: (preset) => stageRef.current?.setView(preset),
		onHideSelected: hideSelected,
		onFocusSelected: focusSelected,
		onIsolateFocusSelected: isolateFocusSelected,
		onLock: () => setLocked((l) => !l),
		onFlyout: (f) => {
			if (f === "animations" && !animation) return;
			setUiHidden(false);
			toggleFlyout(f);
		},
		onScreenshot: () => void screenshot(),
		animation,
	});

	// With no source yet, the card reflects the detail query instead.
	const pendingName =
		kind && id
			? (queryClient
					.getQueryData(studioCatalogQuery(kind, lang).queryKey)
					?.items.find((e) => e.id === id)?.name ?? id)
			: "";
	const loading =
		load.status === "loading"
			? load
			: kind && id && !source && detail.isPending
				? { name: pendingName, loaded: 0, total: 0 }
				: null;
	const showEmpty = !kind && !loading && !shown;
	const loadError =
		load.status === "error" && !loading
			? { ...load, retry: () => setRetry((r) => r + 1) }
			: kind && id && !source && detail.isError
				? {
						name: pendingName,
						message: errorMessage(detail.error),
						retry: () => void detail.refetch(),
					}
				: null;
	const openPicker = () => {
		setUiHidden(false);
		setFlyout("models");
	};

	const stageLabel = shown
		? `3D view of ${shown.name}. ${
				view.panMode
					? "Drag to pan"
					: "Drag to orbit, right-drag or Shift-drag to pan"
			}, scroll to zoom.${view.picking ? " Click a mesh to inspect it." : ""}`
		: "Empty 3D stage";

	return (
		<main
			className="relative h-dvh w-screen select-none overflow-hidden text-foreground"
			style={{ background: background.css }}
		>
			<div className="absolute inset-0">
				<Stage
					animation={animation}
					background={background}
					handleRef={stageRef}
					hiddenParts={hiddenParts}
					label={stageLabel}
					locked={locked}
					model={model}
					onContextLost={setContextLost}
					onFocus={focusSelection}
					onPick={handlePick}
					onPropWarnings={handlePropWarnings}
					onShown={handleShown}
					prefs={prefs}
					view={view}
					zUpMode={prefs.zUp}
				/>
			</div>
			{background.vignette && (
				<div
					aria-hidden="true"
					className="pointer-events-none absolute inset-0 z-10"
					style={{ background: background.vignette }}
				/>
			)}

			{loading && (
				<LoadingCard
					loaded={loading.loaded}
					name={loading.name}
					total={loading.total}
				/>
			)}
			{showEmpty && !uiHidden && flyout !== "models" && (
				<EmptyStage onBrowse={openPicker} />
			)}
			{loadError && !shown && (
				<ErrorStage
					message={loadError.message}
					onBrowse={openPicker}
					onRetry={loadError.retry}
					title={`Couldn't load ${loadError.name}`}
				/>
			)}
			{contextLost && <ContextLostCard />}

			{/* Restores the interface after H. */}
			{uiHidden && (
				<button
					aria-label="Show interface (H)"
					className="absolute end-3 top-3 z-30 flex size-8 items-center justify-center rounded-lg bg-popover/40 text-foreground/70 opacity-30 backdrop-blur transition-opacity hover:opacity-100 focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-ring"
					onClick={() => setUiHidden(false)}
					type="button"
				>
					<EyeIcon className="size-4" />
				</button>
			)}

			<div
				aria-hidden={uiHidden || undefined}
				className={cn(
					"pointer-events-none absolute inset-0 z-20 transition-opacity duration-200",
					uiHidden && "opacity-0",
				)}
				inert={uiHidden || undefined}
			>
				{/* Top-left: back + model identity */}
				<div className="absolute start-3 top-3 flex max-w-[calc(100vw-7.25rem)] flex-col gap-2 sm:start-4 sm:top-4 sm:max-w-md">
					<GlassCard className="pointer-events-auto flex items-center gap-1.5 p-1.5 pe-3">
						<IconButton
							label={
								kind
									? `Back to ${STUDIO_KIND_SINGULAR[kind].toLowerCase()} list`
									: "Back to fluorite.moe"
							}
							render={
								kind ? (
									<Link params={{ kind }} to="/catalog/$kind" />
								) : (
									<Link to="/" />
								)
							}
						>
							<ArrowLeftIcon />
						</IconButton>
						<div className="h-6 w-px bg-border" />
						<div className="flex min-w-0 items-center gap-2 ps-1">
							<LogoMark className="size-4 shrink-0 max-sm:hidden" />
							<div className="flex min-w-0 flex-col">
								<h1 className="truncate font-semibold text-sm leading-tight">
									{name ?? "Studio"}
								</h1>
								<div className="flex min-w-0 items-center gap-1.5 text-muted-foreground text-xs leading-tight">
									{kind && id ? (
										<>
											<span>{STUDIO_KIND_SINGULAR[kind]}</span>
											{source?.entry.rarity !== undefined &&
												source.entry.rarity > 0 && (
													<span className="text-amber-500">
														<span aria-hidden="true">
															{"★".repeat(Math.min(source.entry.rarity, 6))}
														</span>
														<span className="sr-only">
															{source.entry.rarity} star
														</span>
													</span>
												)}
											<span className="truncate font-mono text-[0.6875rem] opacity-80">
												{id}
											</span>
										</>
									) : (
										<span>fluorite.moe model viewer</span>
									)}
								</div>
							</div>
						</div>
					</GlassCard>
					{source && source.entry.tags.length > 0 && (
						<div className="flex flex-wrap gap-1 max-sm:hidden">
							{source.entry.tags.slice(0, 4).map((tag) => (
								<Badge
									className="pointer-events-auto border-border/70 bg-popover/80 text-foreground capitalize backdrop-blur"
									key={tag}
									size="sm"
									variant="outline"
								>
									{tag}
								</Badge>
							))}
						</div>
					)}
					{selected && (
						<SelectionCard
							canIsolate={Boolean(selectedPart)}
							className="pointer-events-auto"
							detailsOpen={view.inspector}
							focused={view.focus !== null && view.focus === selectedMeshId}
							info={selected}
							onClear={() => setViewState({ selection: null })}
							onDetails={() => setViewState({ inspector: !view.inspector })}
							onFocus={focusSelected}
							onHide={hideSelected}
							onIsolateFocus={isolateFocusSelected}
						/>
					)}
					{selected && meshDetail && (
						<InspectorCard
							canIsolate={Boolean(selectedPart)}
							className="pointer-events-auto max-h-[min(52dvh,30rem)]"
							detail={meshDetail}
							focused={view.focus !== null && view.focus === selectedMeshId}
							info={selected}
							onClose={() => setViewState({ inspector: false })}
							onFocus={focusSelected}
							onHide={hideSelected}
							onIsolateFocus={isolateFocusSelected}
						/>
					)}
					{prefs.stats && (
						<StatsCard className="pointer-events-auto" model={shown} />
					)}
				</div>

				{/* Right edge: on-screen camera buttons (touch by default). */}
				{showZoomControls && (
					<div className="absolute end-3 top-1/2 -translate-y-1/2 sm:end-4">
						<ZoomControls
							className="pointer-events-auto"
							enabled={Boolean(shown)}
							locked={locked}
							onPanMode={(panMode) => setViewState({ panMode })}
							onReset={resetAll}
							panMode={view.panMode}
							stage={stageRef}
							touch={touch}
						/>
					</div>
				)}

				{/* Top-right: help, hide UI, theme, settings. Phones keep only hide
				    and settings; help and theme move into the settings panel. */}
				<GlassCard className="pointer-events-auto absolute end-3 top-3 flex items-center gap-0.5 p-1 sm:end-4 sm:top-4">
					<IconButton
						className="max-sm:hidden"
						label="Controls and shortcuts"
						onClick={() => setHelpOpen(true)}
						shortcut="?"
					>
						<CircleHelpIcon />
					</IconButton>
					<IconButton
						label="Hide interface"
						onClick={() => {
							setUiHidden(true);
							setFlyout(null);
						}}
						shortcut="H"
					>
						<EyeOffIcon />
					</IconButton>
					<IconButton
						className="max-sm:hidden"
						label={themeDark ? "Switch to light theme" : "Switch to dark theme"}
						onClick={() =>
							updateSettings({ theme: themeDark ? "light" : "dark" })
						}
					>
						{themeDark ? <SunIcon /> : <MoonIcon />}
					</IconButton>
					<Popover {...flyoutProps("settings")}>
						<IconButton
							aria-pressed={flyout === "settings"}
							label="Settings"
							wrap={(b) => <PopoverTrigger render={b} />}
						>
							<SettingsIcon />
						</IconButton>
						<PopoverPopup
							align="end"
							aria-label="Settings"
							className="rounded-2xl bg-popover/90 backdrop-blur-xl"
							side="bottom"
							sideOffset={8}
						>
							<SettingsPanel
								compact={mobile}
								detectedZUp={shown?.detectedZUp ?? false}
								hasModel={Boolean(shown)}
								onHelp={() => {
									setFlyout(null);
									setHelpOpen(true);
								}}
								onToggleTheme={() =>
									updateSettings({ theme: themeDark ? "light" : "dark" })
								}
								prefs={prefs}
								rigged={Boolean(shown?.rigged)}
								themeDark={themeDark}
							/>
						</PopoverPopup>
					</Popover>
				</GlassCard>

				{/* Left: the animations browser (desktop). Phones get a sheet. */}
				{animation && animationsOpen && !mobile && (
					<div className="pointer-events-none absolute inset-y-0 start-4 flex items-center pt-24 pb-28">
						<AnimationsCard
							className="pointer-events-auto"
							controller={animation}
							onClose={closeAnimations}
							onSelect={selectClip}
							onSelectMoment={selectMoment}
							searchRef={clipSearchRef}
						/>
					</div>
				)}

				{/* Welcome hint */}
				{!prefs.welcomeDismissed && !flyout && (
					<div className="absolute inset-x-0 bottom-36 flex justify-center px-4 sm:bottom-auto sm:start-4 sm:top-24 sm:justify-start sm:px-0">
						<WelcomeHint
							onDismiss={() => setPrefs({ welcomeDismissed: true })}
							touch={touch}
						/>
					</div>
				)}

				{/* Bottom: transport + toolbar */}
				<div className="absolute inset-x-0 bottom-0 flex flex-col items-center gap-2 px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:pb-4">
					{animation && (
						<TransportBar
							className="pointer-events-auto"
							controller={animation}
						/>
					)}
					<GlassCard
						aria-label="Studio tools"
						className="pointer-events-auto flex max-w-full items-center gap-0.5 overflow-x-auto p-1 [scrollbar-width:none]"
						ref={toolbarRef}
						role="toolbar"
					>
						<Flyout
							{...flyoutProps("models")}
							anchor={toolbarRef}
							initialFocus={mobile ? undefined : pickerSearchRef}
							title="Models"
							trigger={
								<ToolbarButton
									active={flyout === "models"}
									icon={<BoxIcon />}
									label="Models"
									shortcut="M"
								/>
							}
						>
							<ModelPicker
								bg={search.bg}
								currentId={id}
								currentKind={kind}
								lang={lang}
								layout={mobile ? "grid" : "strip"}
								onPicked={mobile ? () => setFlyout(null) : undefined}
								onTab={setPickerTab}
								searchRef={pickerSearchRef}
								tab={pickerTab}
							/>
						</Flyout>
						<Flyout
							{...flyoutProps("parts")}
							title="Parts"
							trigger={
								<ToolbarButton
									active={flyout === "parts"}
									icon={<LayersIcon />}
									label="Parts"
									shortcut="P"
								/>
							}
						>
							<PartsPanel
								hidden={hiddenParts}
								model={shown}
								onHiddenChange={setHiddenParts}
								prefs={prefs}
							/>
						</Flyout>
						{animation && mobile && (
							<Flyout
								{...flyoutProps("animations")}
								title="Animations"
								trigger={
									<ToolbarButton
										active={flyout === "animations"}
										icon={<ClapperboardIcon />}
										label="Animations"
										shortcut="A"
									/>
								}
							>
								<AnimationsBrowser
									controller={animation}
									mobile
									onSelect={selectClip}
									onSelectMoment={selectMoment}
								/>
							</Flyout>
						)}
						{animation && !mobile && (
							<ToolbarButton
								active={animationsOpen}
								aria-expanded={animationsOpen}
								icon={<ClapperboardIcon />}
								label="Animations"
								onClick={() => toggleFlyout("animations")}
								ref={animationsButtonRef}
								shortcut="A"
							/>
						)}
						<Flyout
							{...flyoutProps("background")}
							title="Backdrop"
							trigger={
								<ToolbarButton
									active={flyout === "background"}
									icon={<PaletteIcon />}
									label="Backdrop"
									shortcut="B"
								/>
							}
						>
							<BackgroundPanel
								current={backgroundChoice}
								onSelect={selectBackground}
								prefs={prefs}
							/>
						</Flyout>
						<Flyout
							{...flyoutProps("lighting")}
							title="Lighting"
							trigger={
								<ToolbarButton
									active={flyout === "lighting"}
									icon={<SunMediumIcon />}
									label="Lighting"
									shortcut="L"
								/>
							}
						>
							<LightingPanel prefs={prefs} />
						</Flyout>
						<Flyout
							{...flyoutProps("view")}
							title="View"
							trigger={
								<ToolbarButton
									active={
										flyout === "view" ||
										view.picking ||
										view.panMode ||
										view.normals
									}
									icon={<ScanEyeIcon />}
									label="View"
									shortcut="V"
								/>
							}
						>
							<ViewPanel
								hiddenParts={hiddenParts}
								locked={locked}
								model={shown}
								onCapture={() => void screenshot()}
								onFocus={focusSelected}
								onHiddenChange={setHiddenParts}
								onIsolateFocus={isolateFocusSelected}
								onLock={() => setLocked((l) => !l)}
								onReset={resetAll}
								prefs={prefs}
								stage={stageRef}
								touch={touch}
								view={view}
							/>
						</Flyout>
						<div
							aria-hidden="true"
							className="mx-0.5 h-8 w-px shrink-0 bg-border max-sm:hidden"
						/>
						{/* Phones reach Lock and Reset through View and the zoom buttons. */}
						<ToolbarButton
							active={locked}
							aria-pressed={locked}
							className="max-sm:hidden"
							icon={locked ? <LockIcon /> : <UnlockIcon />}
							label={locked ? "Locked" : "Lock"}
							onClick={() => setLocked((l) => !l)}
							shortcut="C"
						/>
						<ToolbarButton
							className="max-sm:hidden"
							hint="Reset view & parts"
							icon={<FocusIcon />}
							label="Reset"
							onClick={resetAll}
							shortcut="F"
						/>
						<ToolbarButton
							disabled={!shown}
							icon={<CameraIcon />}
							label="Capture"
							onClick={() => void screenshot()}
							shortcut="S"
						/>
					</GlassCard>
				</div>
			</div>

			<HelpDialog
				hasClips={Boolean(animation)}
				onOpenChange={setHelpOpen}
				open={helpOpen}
			/>
			<span className="sr-only" aria-live="polite">
				{shown ? `${shown.name} is ready` : ""}
			</span>
		</main>
	);
}
