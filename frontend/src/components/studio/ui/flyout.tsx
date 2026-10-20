/**
 * A Studio flyout: a floating glass popover above the toolbar on desktop, a
 * bottom sheet on phones. Open state is owned by the Studio so only one flyout
 * is open at a time; Base UI handles Escape, outside press and focus return.
 */
import type * as React from "react";
import { glass } from "@/components/studio/ui/chrome";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import {
	Sheet,
	SheetHeader,
	SheetPopup,
	SheetTitle,
	SheetTrigger,
} from "@/components/ui/sheet";
import { cn } from "@/lib/utils";

export interface FlyoutProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	title: string;
	mobile: boolean;
	/** Trigger element; receives the trigger props. */
	trigger: React.ReactElement;
	/** Optional element to position against instead of the trigger. */
	anchor?: React.RefObject<Element | null>;
	className?: string;
	initialFocus?: React.RefObject<HTMLElement | null>;
	children: React.ReactNode;
	side?: "top" | "bottom";
	align?: "start" | "center" | "end";
}

export function Flyout({
	open,
	onOpenChange,
	title,
	mobile,
	trigger,
	anchor,
	className,
	initialFocus,
	children,
	side = "top",
	align = "center",
}: FlyoutProps): React.ReactElement {
	if (mobile) {
		return (
			<Sheet onOpenChange={onOpenChange} open={open}>
				<SheetTrigger render={trigger} />
				<SheetPopup
					className="max-h-[82dvh] rounded-t-2xl pb-[env(safe-area-inset-bottom)]"
					side="bottom"
				>
					<SheetHeader className="pb-2">
						<SheetTitle className="text-base">{title}</SheetTitle>
					</SheetHeader>
					<div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-4">
						{children}
					</div>
				</SheetPopup>
			</Sheet>
		);
	}
	return (
		<Popover onOpenChange={onOpenChange} open={open}>
			<PopoverTrigger render={trigger} />
			<PopoverPopup
				align={align}
				anchor={anchor}
				aria-label={title}
				className={cn(
					glass,
					"rounded-2xl bg-popover/88 dark:bg-popover/82",
					className,
				)}
				initialFocus={initialFocus}
				side={side}
				sideOffset={10}
			>
				{children}
			</PopoverPopup>
		</Popover>
	);
}
