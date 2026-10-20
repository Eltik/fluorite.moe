import type * as React from "react";
import { ShortcutHint } from "@/components/studio/ui/chrome";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipPopup, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export interface ToolbarButtonProps
	extends React.ComponentProps<typeof Button> {
	icon: React.ReactNode;
	label: string;
	shortcut?: string;
	/** Visual "on" state for toggles / open flyouts. */
	active?: boolean;
	/** Tooltip text that replaces the label (e.g. why it is disabled). */
	hint?: string;
}

/**
 * Icon-over-label toolbar button. Forwards all props (including the ref) so
 * it can be a Popover / Sheet trigger.
 */
export function ToolbarButton({
	icon,
	label,
	shortcut,
	active,
	hint,
	className,
	disabled,
	// Trigger wrappers pass `children`; the button renders its own icon + label.
	children: _children,
	...props
}: ToolbarButtonProps): React.ReactElement {
	const button = (
		<Button
			className={cn(
				"h-auto min-w-13 flex-col gap-1 rounded-lg px-2 py-1.5 font-normal text-[0.6875rem] text-muted-foreground leading-none hover:text-foreground max-sm:min-w-11 max-sm:px-1 max-sm:text-[0.625rem] sm:text-[0.6875rem] [&_svg:not([class*='size-'])]:size-4.5",
				active &&
					"bg-primary/12 text-foreground hover:bg-primary/16 dark:bg-primary/20 [&_svg]:text-primary [&_svg]:opacity-100!",
				disabled &&
					"pointer-events-auto cursor-not-allowed opacity-48 hover:bg-transparent",
				className,
			)}
			variant="ghost"
			{...props}
			aria-disabled={disabled || undefined}
			aria-pressed={props["aria-pressed"]}
			onClick={disabled ? (e) => e.preventDefault() : props.onClick}
		/>
	);
	return (
		<Tooltip>
			<TooltipTrigger render={button}>
				{icon}
				<span>{label}</span>
			</TooltipTrigger>
			<TooltipPopup side="top">
				<ShortcutHint
					label={hint ?? label}
					shortcut={disabled ? undefined : shortcut}
				/>
			</TooltipPopup>
		</Tooltip>
	);
}
