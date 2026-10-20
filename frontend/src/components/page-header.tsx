import type * as React from "react";
import { cn } from "@/lib/utils";

export function PageHeader({
	title,
	description,
	actions,
	className,
	icon,
}: {
	title: React.ReactNode;
	description?: React.ReactNode;
	actions?: React.ReactNode;
	icon?: React.ReactNode;
	className?: string;
}): React.ReactElement {
	return (
		<div
			className={cn(
				"flex flex-wrap items-end justify-between gap-x-6 gap-y-3",
				className,
			)}
		>
			<div className="flex min-w-0 items-center gap-3">
				{icon && (
					<div className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-card text-muted-foreground shadow-xs/5 [&_svg]:size-4.5">
						{icon}
					</div>
				)}
				<div className="min-w-0">
					<h1 className="truncate font-heading font-semibold text-lg tracking-tight">
						{title}
					</h1>
					{description && (
						<p className="text-pretty text-muted-foreground text-sm">
							{description}
						</p>
					)}
				</div>
			</div>
			{actions && (
				<div className="flex min-w-0 max-w-full flex-wrap items-center gap-2">
					{actions}
				</div>
			)}
		</div>
	);
}
