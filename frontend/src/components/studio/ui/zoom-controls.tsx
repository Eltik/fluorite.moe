/**
 * On-screen camera buttons near the stage edge: zoom in / out, reset and,
 * on touch devices, the pan-mode toggle. Shown on touch devices by default
 * and on request elsewhere (prefs.zoomControls).
 */
import { FocusIcon, HandIcon, ZoomInIcon, ZoomOutIcon } from "lucide-react";
import type * as React from "react";
import type { StageHandle } from "@/components/studio/stage";
import { GlassCard, IconButton } from "@/components/studio/ui/chrome";
import { ZOOM_STEP } from "@/components/studio/view-math";
import { cn } from "@/lib/utils";

export function ZoomControls({
	stage,
	enabled,
	locked,
	touch,
	panMode,
	onPanMode,
	onReset,
	className,
}: {
	stage: React.RefObject<StageHandle | null>;
	/** A model is shown. */
	enabled: boolean;
	locked: boolean;
	touch: boolean;
	panMode: boolean;
	onPanMode: (on: boolean) => void;
	/** Reset the view and the shown parts. */
	onReset: () => void;
	className?: string;
}): React.ReactElement {
	const canMove = enabled && !locked;
	return (
		<GlassCard
			aria-label="Camera"
			className={cn("flex flex-col items-center gap-0.5 p-1", className)}
			role="toolbar"
		>
			<IconButton
				disabled={!canMove}
				label="Zoom in"
				onClick={() => stage.current?.zoomBy(ZOOM_STEP)}
				shortcut="="
				tooltipSide="left"
			>
				<ZoomInIcon />
			</IconButton>
			<IconButton
				disabled={!canMove}
				label="Zoom out"
				onClick={() => stage.current?.zoomBy(1 / ZOOM_STEP)}
				shortcut="−"
				tooltipSide="left"
			>
				<ZoomOutIcon />
			</IconButton>
			<div aria-hidden="true" className="my-0.5 h-px w-5 bg-border" />
			<IconButton
				disabled={!enabled}
				label="Reset view & parts"
				onClick={onReset}
				shortcut="F"
				tooltipSide="left"
			>
				<FocusIcon />
			</IconButton>
			{touch && (
				<IconButton
					aria-pressed={panMode}
					className={cn(panMode && "bg-primary/12 text-primary")}
					disabled={!canMove}
					label={panMode ? "Pan mode on: one finger pans" : "Pan mode"}
					onClick={() => onPanMode(!panMode)}
					tooltipSide="left"
				>
					<HandIcon />
				</IconButton>
			)}
		</GlassCard>
	);
}
