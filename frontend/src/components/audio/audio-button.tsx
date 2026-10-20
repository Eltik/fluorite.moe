import { CircleAlertIcon, PauseIcon, PlayIcon } from "lucide-react";
import type * as React from "react";
import { Button, type ButtonProps } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import type { AudioSources } from "@/lib/audio";
import { cn } from "@/lib/utils";
import { type AudioTrack, audioEngine, useTrackStatus } from "./engine";

function sourceKey(src: AudioSources | string): string {
	return typeof src === "string" ? src : (src.ogg ?? src.m4a ?? "");
}

/**
 * Play / pause toggle for one clip, sharing the app-wide player (starting it
 * stops anything else that is playing).
 *
 * ```tsx
 * <AudioButton src={line.audio.EN} title={line.title} subtitle="Chen Qianyu · EN" />
 * ```
 *
 * `src` is the `{ ogg, m4a }` object the API returns (the browser's best
 * format is picked) or a single URL. Renders disabled when `src` is missing.
 */
export function AudioButton({
	src,
	title,
	subtitle,
	label,
	size = "icon-sm",
	variant = "ghost",
	className,
}: {
	src: AudioSources | string | undefined | null;
	/** Shown by the player bar while this clip is current. */
	title?: string;
	subtitle?: string;
	/** Accessible name; defaults to "Play {title}". */
	label?: string;
	size?: Extract<ButtonProps["size"], `icon${string}`>;
	variant?: ButtonProps["variant"];
	className?: string;
}): React.ReactElement {
	const key = src ? sourceKey(src) : "";
	const status = useTrackStatus(key);
	const active = status === "playing" || status === "loading";
	const name = label ?? (title ? `Play ${title}` : "Play clip");

	const track: AudioTrack | null = src ? { key, src, title, subtitle } : null;

	return (
		<Button
			aria-label={active ? `Pause ${title ?? "clip"}` : name}
			aria-pressed={active}
			className={cn(
				"rounded-full",
				active && "text-primary",
				status === "error" && "text-destructive-foreground",
				className,
			)}
			disabled={!track}
			onClick={(event) => {
				event.stopPropagation();
				if (track) audioEngine.toggle(track);
			}}
			size={size}
			title={status === "error" ? "Playback failed — retry" : undefined}
			variant={variant}
		>
			{status === "loading" ? (
				<Spinner />
			) : status === "playing" ? (
				<PauseIcon className="fill-current" />
			) : status === "error" ? (
				<CircleAlertIcon />
			) : (
				<PlayIcon className="fill-current" />
			)}
		</Button>
	);
}
