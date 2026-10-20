import {
	CircleAlertIcon,
	DownloadIcon,
	PauseIcon,
	PlayIcon,
	SkipBackIcon,
	SkipForwardIcon,
} from "lucide-react";
import type * as React from "react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { audioSrc, formatDuration } from "@/lib/audio";
import { cn } from "@/lib/utils";
import { audioEngine, useAudioState } from "./engine";

/**
 * Compact transport for the shared player: previous / play-pause / next,
 * a scrubber with elapsed and total time, and a download link.
 */
export function PlayerBar({
	onPrevious,
	onNext,
	fallbackDuration,
	className,
	emptyHint,
}: {
	onPrevious?: () => void;
	onNext?: () => void;
	/** Duration to show before the media reports one (e.g. from the index). */
	fallbackDuration?: number;
	className?: string;
	emptyHint?: React.ReactNode;
}): React.ReactElement {
	const s = useAudioState();
	const duration = s.duration || fallbackDuration || 0;
	const busy = s.status === "loading";
	const playing = s.status === "playing" || busy;
	const href = s.track ? audioSrc(s.track.src) : undefined;

	return (
		<div
			className={cn(
				"flex items-center gap-2 rounded-xl border bg-card px-2 py-1.5 shadow-xs/5 sm:gap-3 sm:px-3",
				className,
			)}
		>
			<div className="flex shrink-0 items-center">
				<Button
					aria-label="Previous clip"
					disabled={!onPrevious}
					onClick={onPrevious}
					size="icon-sm"
					variant="ghost"
				>
					<SkipBackIcon className="fill-current" />
				</Button>
				<Button
					aria-label={playing ? "Pause" : "Play"}
					className="rounded-full"
					disabled={!s.track}
					onClick={() => audioEngine.toggle()}
					size="icon"
					variant="default"
				>
					{busy ? (
						<Spinner />
					) : playing ? (
						<PauseIcon className="fill-current" />
					) : (
						<PlayIcon className="fill-current" />
					)}
				</Button>
				<Button
					aria-label="Next clip"
					disabled={!onNext}
					onClick={onNext}
					size="icon-sm"
					variant="ghost"
				>
					<SkipForwardIcon className="fill-current" />
				</Button>
			</div>

			<div className="flex min-w-0 flex-1 flex-col gap-1">
				<div className="flex min-w-0 items-baseline gap-2 text-sm">
					{s.track ? (
						<>
							<span className="truncate font-medium">
								{s.track.title ?? s.track.key}
							</span>
							{s.track.subtitle && (
								<span className="hidden truncate text-muted-foreground text-xs sm:inline">
									{s.track.subtitle}
								</span>
							)}
							{s.status === "error" && (
								<span className="flex shrink-0 items-center gap-1 text-destructive-foreground text-xs">
									<CircleAlertIcon className="size-3.5" />
									{s.error}
								</span>
							)}
						</>
					) : (
						<span className="truncate text-muted-foreground">
							{emptyHint ?? "Nothing playing"}
						</span>
					)}
				</div>
				<div className="flex items-center gap-2">
					<span className="w-9 shrink-0 text-right font-mono text-muted-foreground text-xs tabular-nums">
						{formatDuration(s.track ? s.time : undefined)}
					</span>
					<Slider
						aria-label="Seek"
						className="[&_[data-slot=slider-control]]:min-w-0"
						disabled={!s.track || duration <= 0}
						max={Math.max(duration, 0.001)}
						min={0}
						onValueChange={(v) => audioEngine.seek(Array.isArray(v) ? v[0] : v)}
						step={0.01}
						value={Math.min(s.time, Math.max(duration, 0.001))}
					/>
					<span className="w-9 shrink-0 font-mono text-muted-foreground text-xs tabular-nums">
						{formatDuration(s.track ? duration : undefined)}
					</span>
				</div>
			</div>

			<div className="hidden shrink-0 items-center gap-1 text-muted-foreground text-xs lg:flex">
				<Kbd>Space</Kbd>
				<span>play</span>
				<Kbd>N</Kbd>
				<Kbd>P</Kbd>
				<span>next / prev</span>
			</div>
			<Button
				aria-label="Download clip"
				disabled={!href}
				render={href ? <a download href={href} /> : undefined}
				size="icon-sm"
				variant="ghost"
			>
				<DownloadIcon />
			</Button>
		</div>
	);
}
