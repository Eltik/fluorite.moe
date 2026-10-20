/** Shared building blocks for the Studio overlay: glass cards and buttons. */
import type * as React from "react";
import { Button, type ButtonProps } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipPopup, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** Floating translucent card that reads well over any backdrop. */
export const glass =
	"rounded-xl border border-border/70 bg-popover/80 text-popover-foreground shadow-lg/8 backdrop-blur-xl backdrop-saturate-150 dark:bg-popover/72";

export function GlassCard({
	className,
	...props
}: React.ComponentProps<"div">): React.ReactElement {
	return <div className={cn(glass, className)} {...props} />;
}

export function ShortcutHint({
	label,
	shortcut,
}: {
	label: React.ReactNode;
	shortcut?: string;
}): React.ReactElement {
	return (
		<span className="flex items-center gap-2">
			{label}
			{shortcut && (
				<Kbd className="h-4.5 min-w-4.5 text-[0.625rem]">{shortcut}</Kbd>
			)}
		</span>
	);
}

export interface IconButtonProps extends ButtonProps {
	label: string;
	shortcut?: string;
	tooltipSide?: "top" | "bottom" | "left" | "right";
	/** Wraps the button (e.g. a PopoverTrigger) while keeping the tooltip. */
	wrap?: (button: React.ReactElement) => React.ReactElement;
}

/** Icon-only button with an accessible label and a tooltip. */
export function IconButton({
	label,
	shortcut,
	tooltipSide = "bottom",
	className,
	children,
	wrap,
	...props
}: IconButtonProps): React.ReactElement {
	const button = (
		<Button
			aria-label={label}
			className={cn("rounded-lg", className)}
			size="icon-sm"
			variant="ghost"
			{...props}
		/>
	);
	return (
		<Tooltip>
			<TooltipTrigger render={wrap ? wrap(button) : button}>
				{children}
			</TooltipTrigger>
			<TooltipPopup side={tooltipSide}>
				<ShortcutHint label={label} shortcut={shortcut} />
			</TooltipPopup>
		</Tooltip>
	);
}

/** Section heading inside flyouts. */
export function PanelHeading({
	children,
	action,
}: {
	children: React.ReactNode;
	action?: React.ReactNode;
}): React.ReactElement {
	return (
		<div className="flex min-h-7 items-center justify-between gap-2">
			<h3 className="font-medium text-muted-foreground text-xs uppercase tracking-wide">
				{children}
			</h3>
			{action}
		</div>
	);
}

export function formatMB(bytes: number): string {
	const mb = bytes / (1024 * 1024);
	return mb >= 10 ? mb.toFixed(0) : mb.toFixed(1);
}
