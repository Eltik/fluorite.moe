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
import { type LoadedModel, loadModel } from "@/components/studio/load-model";
import {
	type BackgroundId,
	isHexColor,
	setPrefs,
	usePrefs,
} from "@/components/studio/prefs";
import { BACKGROUNDS, resolveBackground } from "@/components/studio/presets";
import { Stage, type StageHandle } from "@/components/studio/stage";
import { GlassCard, IconButton } from "@/components/studio/ui/chrome";
import { Flyout } from "@/components/studio/ui/flyout";
import { ModelPicker } from "@/components/studio/ui/model-picker";
import {
	ContextLostCard,
	EmptyStage,
	ErrorStage,
	HelpDialog,
	LoadingCard,
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
import { ClipList, TransportBar } from "@/components/studio/ui/transport";
import { useStudioShortcuts } from "@/components/studio/use-shortcuts";
import { Badge } from "@/components/ui/badge";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import { toastManager } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useMediaQuery } from "@/hooks/use-media-query";
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
	const source: StudioModelSource | undefined =
		kind && id ? detail.data : undefined;

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
				setHiddenParts(
					new Set(
						loaded.parts.filter((p) => p.hiddenByDefault).map((p) => p.id),
					),
				);
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
		return () => controller.abort();
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
		};
	}, []);

	// ---------------------------------------------------------- animation

	const animation = React.useMemo(
		() =>
			shown?.animations.length
				? new AnimationController(shown.content, shown.animations)
				: null,
		[shown],
	);
	React.useEffect(() => () => animation?.dispose(), [animation]);

	// `?clip=` deep link once the clips are known. Only on a new controller:
	// later URL changes come from selecting clips.
	// biome-ignore lint/correctness/useExhaustiveDependencies: see above
	React.useEffect(() => {
		if (!animation) return;
		const wanted = search.clip;
		if (wanted && animation.select(wanted)) return;
		const first = animation.clips[0];
		if (first) animation.select(first.name, false);
	}, [animation]);

	const selectClip = React.useCallback(
		(name: string) => {
			animation?.select(name);
			void navigate({
				to: ".",
				search: (prev: StudioSearch) => ({ ...prev, clip: name }),
				replace: true,
			} as never);
		},
		[animation, navigate],
	);

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
	const flyoutProps = (f: FlyoutId) => ({
		open: flyout === f && !uiHidden,
		onOpenChange: (open: boolean) =>
			setFlyout((cur) => (open ? f : cur === f ? null : cur)),
		mobile,
	});

	const screenshot = React.useCallback(async () => {
		try {
			const base = shown ? `${shown.key.replace("/", "-")}` : "studio";
			await stageRef.current?.screenshot(`fluorite-${base}.png`);
		} catch (error) {
			toastManager.add({
				type: "error",
				title: "Screenshot failed",
				description: errorMessage(error),
			});
		}
	}, [shown]);

	useStudioShortcuts({
		enabled: !helpOpen,
		onHelp: () => setHelpOpen((o) => !o),
		onToggleUi: () => {
			setUiHidden((h) => !h);
			setFlyout(null);
		},
		onShowUi: () => setUiHidden(false),
		uiHidden,
		onReset: () => stageRef.current?.resetView(),
		onLock: () => setLocked((l) => !l),
		onFlyout: (f) => {
			if (f === "animations" && !animation) return;
			setUiHidden(false);
			toggleFlyout(f);
		},
		onScreenshot: () => void screenshot(),
		animation,
	});

	const loading = load.status === "loading" ? load : null;
	const showEmpty = !kind && !loading && !shown;
	const loadError = load.status === "error" && !loading ? load : null;
	const openPicker = () => {
		setUiHidden(false);
		setFlyout("models");
	};

	const stageLabel = shown
		? `3D view of ${shown.name}. Drag to orbit, right-drag to pan, scroll to zoom.`
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
					onShown={handleShown}
					prefs={prefs}
					zUpMode={prefs.zUp}
				/>
			</div>

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
					onRetry={() => setRetry((r) => r + 1)}
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
					{prefs.stats && (
						<StatsCard className="pointer-events-auto" model={shown} />
					)}
				</div>

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
							/>
						</Flyout>
						{animation && (
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
								<ClipList controller={animation} onSelect={selectClip} />
							</Flyout>
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
						<div
							aria-hidden="true"
							className="mx-0.5 h-8 w-px shrink-0 bg-border max-sm:hidden"
						/>
						<ToolbarButton
							active={locked}
							aria-pressed={locked}
							icon={locked ? <LockIcon /> : <UnlockIcon />}
							label={locked ? "Locked" : "Lock"}
							onClick={() => setLocked((l) => !l)}
							shortcut="C"
						/>
						<ToolbarButton
							icon={<FocusIcon />}
							label="Reset"
							onClick={() => stageRef.current?.resetView()}
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
