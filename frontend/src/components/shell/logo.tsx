import type * as React from "react";
import { cn } from "@/lib/utils";

/** Octahedral fluorite crystal mark. */
export function LogoMark({
	className,
	...props
}: React.ComponentProps<"svg">): React.ReactElement {
	return (
		<svg
			aria-hidden="true"
			className={cn("size-5", className)}
			fill="none"
			viewBox="0 0 24 24"
			{...props}
		>
			<path d="M12 1.5 3 11l7.5 3.2L12 1.5Z" fill="#a78bfa" />
			<path d="M12 1.5 21 11l-10.5 3.2L12 1.5Z" fill="#8b5cf6" />
			<path d="M3 11l7.5 3.2L12 22.5 3 11Z" fill="#7c3aed" />
			<path d="M21 11l-10.5 3.2L12 22.5 21 11Z" fill="#5b21b6" />
			<path
				d="M12 1.5 3 11l9 11.5L21 11 12 1.5Zm0 0-1.5 12.7M3 11l7.5 3.2L21 11m-10.5 3.2L12 22.5"
				stroke="currentColor"
				strokeLinejoin="round"
				strokeOpacity={0.2}
				strokeWidth={0.75}
			/>
		</svg>
	);
}

export function Wordmark({ className }: { className?: string }) {
	return (
		<span
			className={cn(
				"font-semibold text-[0.9375rem] text-foreground tracking-tight",
				className,
			)}
		>
			fluorite
			<span className="font-normal text-muted-foreground">.moe</span>
		</span>
	);
}
