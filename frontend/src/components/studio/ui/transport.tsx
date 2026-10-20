/** Animation transport bar and the searchable, virtualized clip list. */
import { useVirtualizer } from "@tanstack/react-virtual";
import {
	PauseIcon,
	PlayIcon,
	Repeat1Icon,
	RepeatIcon,
	SearchIcon,
	StepBackIcon,
	StepForwardIcon,
} from "lucide-react";
import * as React from "react";
import {
	type AnimationController,
	type AnimationState,
	SPEEDS,
} from "@/components/studio/animation";
import { GlassCard, IconButton } from "@/components/studio/ui/chrome";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Tooltip, TooltipPopup, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const IDLE: AnimationState = {
	clip: null,
	playing: false,
	speed: 1,
	loop: true,
	duration: 0,
};

export function useAnimationState(
	controller: AnimationController | null,
): AnimationState {
	return React.useSyncExternalStore(
		controller?.subscribe ?? noopSubscribe,
		controller?.getSnapshot ?? idleSnapshot,
		idleSnapshot,
	);
}

function noopSubscribe(): () => void {
	return () => {};
}
function idleSnapshot(): AnimationState {
	return IDLE;
}

function formatTime(t: number): string {
	const m = Math.floor(t / 60);
	const s = t - m * 60;
	return `${m}:${s.toFixed(1).padStart(4, "0")}`;
}

/** Polls the mixer time each frame while playing (no React state churn otherwise). */
function useClipTime(
	controller: AnimationController | null,
	state: AnimationState,
): number {
	const [time, setTime] = React.useState(0);
	React.useEffect(() => {
		if (!controller) {
			setTime(0);
			return;
		}
		setTime(controller.time);
		const unsubscribe = controller.subscribe(() => setTime(controller.time));
		if (!state.playing) return unsubscribe;
		let frame = 0;
		const tick = () => {
			setTime(controller.time);
			frame = requestAnimationFrame(tick);
		};
		frame = requestAnimationFrame(tick);
		return () => {
			cancelAnimationFrame(frame);
			unsubscribe();
		};
	}, [controller, state.playing]);
	return time;
}

export function TransportBar({
	controller,
	className,
}: {
	controller: AnimationController | null;
	className?: string;
}): React.ReactElement {
	const state = useAnimationState(controller);
	const time = useClipTime(controller, state);
	const enabled = Boolean(controller?.clips.length);
	const ready = enabled && state.clip !== null;

	const bar = (
		<GlassCard
			aria-disabled={!enabled || undefined}
			aria-label="Animation playback"
			className={cn(
				"flex h-10 w-full max-w-[min(36rem,100%)] items-center gap-0.5 px-1 sm:w-auto sm:gap-1 sm:px-1.5",
				!enabled && "opacity-60",
				className,
			)}
			role="toolbar"
			tabIndex={enabled ? undefined : 0}
		>
			<IconButton
				disabled={!ready}
				label="Previous frame"
				onClick={() => controller?.step(-1)}
				shortcut="←"
				tooltipSide="top"
			>
				<StepBackIcon />
			</IconButton>
			<IconButton
				disabled={!enabled}
				label={state.playing ? "Pause" : "Play"}
				onClick={() => controller?.toggle()}
				shortcut="Space"
				tooltipSide="top"
			>
				{state.playing ? <PauseIcon /> : <PlayIcon />}
			</IconButton>
			<IconButton
				disabled={!ready}
				label="Next frame"
				onClick={() => controller?.step(1)}
				shortcut="→"
				tooltipSide="top"
			>
				<StepForwardIcon />
			</IconButton>
			<Button
				aria-label={`Playback speed ${state.speed}×`}
				className="w-11 font-mono text-xs tabular-nums"
				disabled={!enabled}
				onClick={() => {
					const i = SPEEDS.indexOf(state.speed as (typeof SPEEDS)[number]);
					controller?.setSpeed(SPEEDS[(i + 1) % SPEEDS.length]);
				}}
				size="sm"
				variant="ghost"
			>
				{state.speed}×
			</Button>
			<IconButton
				aria-pressed={state.loop}
				disabled={!enabled}
				label={state.loop ? "Looping" : "Play once"}
				onClick={() => controller?.setLoop(!state.loop)}
				tooltipSide="top"
			>
				{state.loop ? <RepeatIcon /> : <Repeat1Icon />}
			</IconButton>
			<div className="mx-2 min-w-0 flex-1 sm:w-40 sm:flex-none">
				<Slider
					aria-label="Seek"
					getAriaLabel={() => "Seek"}
					className="[&_[data-slot=slider-control]]:min-w-0"
					disabled={!ready}
					max={Math.max(state.duration, 0.001)}
					min={0}
					onValueChange={(v) => controller?.seek(Array.isArray(v) ? v[0] : v)}
					step={0.001}
					value={ready ? Math.min(time, state.duration) : 0}
				/>
			</div>
			<span className="min-w-[7.5rem] whitespace-nowrap pe-1.5 text-end font-mono text-muted-foreground text-xs tabular-nums max-sm:hidden">
				{formatTime(ready ? time : 0)} / {formatTime(state.duration)}
			</span>
		</GlassCard>
	);

	if (enabled) return bar;
	return (
		<Tooltip>
			<TooltipTrigger render={bar} />
			<TooltipPopup side="top">Animations aren't exported yet</TooltipPopup>
		</Tooltip>
	);
}

export function ClipList({
	controller,
	onSelect,
}: {
	controller: AnimationController;
	onSelect: (name: string) => void;
}): React.ReactElement {
	const state = useAnimationState(controller);
	const [search, setSearch] = React.useState("");
	const deferred = React.useDeferredValue(search);
	const scrollRef = React.useRef<HTMLDivElement>(null);
	const clips = React.useMemo(() => {
		const needle = deferred.trim().toLowerCase();
		return needle
			? controller.clips.filter((c) => c.name.toLowerCase().includes(needle))
			: controller.clips;
	}, [controller, deferred]);

	const virtualizer = useVirtualizer({
		count: clips.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: () => 36,
		overscan: 8,
	});

	React.useEffect(() => {
		const i = clips.findIndex((c) => c.name === state.clip);
		if (i >= 0) virtualizer.scrollToIndex(i, { align: "center" });
	}, [clips, state.clip, virtualizer]);

	return (
		<div className="flex w-full flex-col gap-2 sm:w-80">
			<div className="relative">
				<SearchIcon
					aria-hidden="true"
					className="pointer-events-none absolute start-2.5 top-1/2 z-10 size-4 -translate-y-1/2 text-muted-foreground"
				/>
				<Input
					aria-label="Search animations"
					className="ps-8"
					onChange={(e) => setSearch(e.target.value)}
					placeholder={`Search ${controller.clips.length} animations…`}
					size="sm"
					type="search"
					value={search}
				/>
			</div>
			<div
				className="relative h-[min(22rem,45dvh)] overflow-y-auto"
				ref={scrollRef}
			>
				{clips.length === 0 ? (
					<p className="py-6 text-center text-muted-foreground text-sm">
						No match
					</p>
				) : (
					<ul
						aria-label="Animations"
						className="relative w-full"
						style={{ height: virtualizer.getTotalSize() }}
					>
						{virtualizer.getVirtualItems().map((row) => {
							const clip = clips[row.index];
							const active = clip.name === state.clip;
							return (
								<li
									className="absolute inset-x-0"
									key={clip.uuid}
									style={{
										height: row.size,
										transform: `translateY(${row.start}px)`,
									}}
								>
									<button
										aria-current={active ? "true" : undefined}
										className={cn(
											"flex h-full w-full items-center gap-2 rounded-lg px-2.5 text-start text-sm outline-none hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-ring",
											active && "bg-primary/10 font-medium",
										)}
										onClick={() => onSelect(clip.name)}
										type="button"
									>
										<span className="min-w-0 flex-1 truncate" title={clip.name}>
											{clip.name}
										</span>
										<span className="font-mono text-muted-foreground text-xs tabular-nums">
											{clip.duration.toFixed(1)}s
										</span>
									</button>
								</li>
							);
						})}
					</ul>
				)}
			</div>
		</div>
	);
}
