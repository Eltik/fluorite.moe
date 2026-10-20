import { StarIcon } from "lucide-react";
import type * as React from "react";
import { rarityColor } from "@/lib/catalog";
import { cn } from "@/lib/utils";

/** Filled stars in the rarity colour (contrast-adjusted for light mode). */
export function RarityStars({
	rarity,
	className,
	size = "sm",
}: {
	rarity: number | null | undefined;
	className?: string;
	size?: "sm" | "md";
}): React.ReactElement | null {
	if (!rarity) return null;
	const color = rarityColor(rarity);
	return (
		<span
			aria-label={`${rarity} star${rarity === 1 ? "" : "s"}`}
			className={cn(
				"inline-flex items-center",
				size === "sm" ? "gap-px" : "gap-0.5",
				"[color:oklch(from_var(--rarity)_min(l,0.62)_c_h)] dark:[color:var(--rarity)]",
				className,
			)}
			role="img"
			style={{ "--rarity": color } as React.CSSProperties}
		>
			{Array.from({ length: rarity }, (_, i) => (
				<StarIcon
					aria-hidden
					className={cn(
						"fill-current stroke-none",
						size === "sm" ? "size-3" : "size-4",
					)}
					// biome-ignore lint/suspicious/noArrayIndexKey: identical glyphs
					key={i}
				/>
			))}
		</span>
	);
}

/** Inline style that exposes the rarity colour as `--rarity`. */
export function rarityStyle(
	rarity: number | null | undefined,
): React.CSSProperties | undefined {
	const color = rarityColor(rarity);
	return color ? ({ "--rarity": color } as React.CSSProperties) : undefined;
}
