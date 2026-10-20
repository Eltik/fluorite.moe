import {
	DownloadIcon,
	ImageOffIcon,
	MaximizeIcon,
	ZoomInIcon,
	ZoomOutIcon,
} from "lucide-react";
import * as React from "react";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";

const MIN_SCALE = 0.05;
const MAX_SCALE = 32;

interface View {
	scale: number;
	x: number;
	y: number;
}

/**
 * Zoomable, pannable image on a checkerboard. Wheel / pinch-zoom around the
 * cursor, drag to pan, double-click toggles fit and 1:1. Keyboard: + - 0 1.
 */
export function ImageViewer({
	src,
	alt,
	downloadName,
	bytes,
	className,
}: {
	src: string;
	alt: string;
	downloadName?: string;
	bytes?: number;
	className?: string;
}): React.ReactElement {
	const frameRef = React.useRef<HTMLDivElement>(null);
	const [natural, setNatural] = React.useState<{ w: number; h: number } | null>(
		null,
	);
	const [failed, setFailed] = React.useState(false);
	const [view, setView] = React.useState<View>({ scale: 1, x: 0, y: 0 });
	const [fitScale, setFitScale] = React.useState(1);
	const drag = React.useRef<{
		x: number;
		y: number;
		vx: number;
		vy: number;
	} | null>(null);

	// biome-ignore lint/correctness/useExhaustiveDependencies: reset per image
	React.useEffect(() => {
		setNatural(null);
		setFailed(false);
	}, [src]);

	const fit = React.useCallback(() => {
		const frame = frameRef.current;
		if (!frame || !natural) return;
		const scale = Math.min(
			1,
			(frame.clientWidth - 32) / natural.w,
			(frame.clientHeight - 32) / natural.h,
		);
		setFitScale(scale);
		setView({ scale, x: 0, y: 0 });
	}, [natural]);

	React.useLayoutEffect(() => {
		fit();
	}, [fit]);

	React.useEffect(() => {
		const frame = frameRef.current;
		if (!frame) return;
		const observer = new ResizeObserver(() => fit());
		observer.observe(frame);
		return () => observer.disconnect();
	}, [fit]);

	const zoomAt = React.useCallback(
		(factor: number, clientX?: number, clientY?: number) => {
			const frame = frameRef.current;
			setView((v) => {
				const scale = Math.min(
					MAX_SCALE,
					Math.max(MIN_SCALE, v.scale * factor),
				);
				if (!frame || clientX === undefined || clientY === undefined) {
					return { ...v, scale };
				}
				const rect = frame.getBoundingClientRect();
				// Keep the point under the cursor fixed.
				const px = clientX - rect.left - rect.width / 2;
				const py = clientY - rect.top - rect.height / 2;
				const k = scale / v.scale;
				return { scale, x: px - (px - v.x) * k, y: py - (py - v.y) * k };
			});
		},
		[],
	);

	// Non-passive wheel listener so the page does not scroll while zooming.
	React.useEffect(() => {
		const frame = frameRef.current;
		if (!frame) return;
		const onWheel = (event: WheelEvent) => {
			event.preventDefault();
			const factor = Math.exp(-event.deltaY * (event.ctrlKey ? 0.01 : 0.0015));
			zoomAt(factor, event.clientX, event.clientY);
		};
		frame.addEventListener("wheel", onWheel, { passive: false });
		return () => frame.removeEventListener("wheel", onWheel);
	}, [zoomAt]);

	const onKeyDown = (event: React.KeyboardEvent) => {
		if (event.key === "+" || event.key === "=") zoomAt(1.25);
		else if (event.key === "-") zoomAt(0.8);
		else if (event.key === "0") fit();
		else if (event.key === "1") setView({ scale: 1, x: 0, y: 0 });
		else return;
		event.preventDefault();
	};

	return (
		<div className={cn("flex min-h-0 flex-1 flex-col", className)}>
			<div className="flex h-9 shrink-0 items-center gap-1 border-b px-2 text-muted-foreground text-xs">
				<span className="px-1 tabular-nums">
					{natural ? `${natural.w} × ${natural.h}` : "—"}
					{bytes !== undefined && ` · ${formatBytes(bytes)}`}
				</span>
				<span className="ms-auto w-12 text-end tabular-nums">
					{Math.round(view.scale * 100)}%
				</span>
				<Button
					aria-label="Zoom out"
					onClick={() => zoomAt(0.8)}
					size="icon-sm"
					variant="ghost"
				>
					<ZoomOutIcon />
				</Button>
				<Button
					aria-label="Zoom in"
					onClick={() => zoomAt(1.25)}
					size="icon-sm"
					variant="ghost"
				>
					<ZoomInIcon />
				</Button>
				<Button
					aria-label="Fit to view"
					onClick={fit}
					size="icon-sm"
					variant="ghost"
				>
					<MaximizeIcon />
				</Button>
				<Button
					onClick={() => setView({ scale: 1, x: 0, y: 0 })}
					size="xs"
					variant="ghost"
				>
					1:1
				</Button>
				{downloadName && (
					<Button
						aria-label="Download image"
						render={<a download={downloadName} href={src} />}
						size="icon-sm"
						variant="ghost"
					>
						<DownloadIcon />
					</Button>
				)}
			</div>
			<div
				aria-label={`${alt}. Scroll to zoom, drag to pan, 0 to fit, 1 for actual size.`}
				className="relative min-h-0 flex-1 cursor-grab touch-none select-none overflow-hidden bg-checker outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset active:cursor-grabbing"
				onDoubleClick={() =>
					Math.abs(view.scale - fitScale) < 0.001
						? setView({ scale: 1, x: 0, y: 0 })
						: fit()
				}
				onKeyDown={onKeyDown}
				onPointerDown={(event) => {
					event.currentTarget.setPointerCapture(event.pointerId);
					drag.current = {
						x: event.clientX,
						y: event.clientY,
						vx: view.x,
						vy: view.y,
					};
				}}
				onPointerMove={(event) => {
					const d = drag.current;
					if (!d) return;
					setView((v) => ({
						...v,
						x: d.vx + event.clientX - d.x,
						y: d.vy + event.clientY - d.y,
					}));
				}}
				onPointerUp={() => {
					drag.current = null;
				}}
				ref={frameRef}
				role="img"
				// biome-ignore lint/a11y/noNoninteractiveTabindex: keyboard zoom controls
				tabIndex={0}
			>
				{failed ? (
					<div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-background text-muted-foreground text-sm">
						<ImageOffIcon className="size-5" />
						Could not load this image.
					</div>
				) : (
					<>
						{!natural && (
							<div className="absolute inset-0 flex items-center justify-center">
								<Spinner />
							</div>
						)}
						<img
							alt={alt}
							className={cn(
								"absolute top-1/2 left-1/2 max-w-none",
								!natural && "opacity-0",
								view.scale > 2 && "[image-rendering:pixelated]",
							)}
							decoding="async"
							draggable={false}
							onError={() => setFailed(true)}
							onLoad={(event) =>
								setNatural({
									w: event.currentTarget.naturalWidth,
									h: event.currentTarget.naturalHeight,
								})
							}
							src={src}
							style={{
								width: natural?.w,
								height: natural?.h,
								transform: `translate(-50%, -50%) translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
							}}
						/>
					</>
				)}
			</div>
		</div>
	);
}
