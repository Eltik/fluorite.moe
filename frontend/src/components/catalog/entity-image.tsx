import { ImageOffIcon } from "lucide-react";
import * as React from "react";
import { fileUrl, type ThumbSize, thumbUrl } from "@/lib/api";
import { cn } from "@/lib/utils";

/**
 * Texture image with a graceful fallback (initials or an icon) for missing
 * paths and failed loads. `size` picks a thumbnail; omit it for the full PNG.
 */
export function EntityImage({
	path,
	size,
	alt = "",
	name,
	className,
	imgClassName,
	eager = false,
}: {
	path: string | null | undefined;
	size?: ThumbSize;
	alt?: string;
	/** Used for the initials fallback. */
	name?: string;
	className?: string;
	imgClassName?: string;
	eager?: boolean;
}): React.ReactElement {
	const [failed, setFailed] = React.useState(false);
	// biome-ignore lint/correctness/useExhaustiveDependencies: reset when the source changes
	React.useEffect(() => setFailed(false), [path]);
	const src = path
		? size
			? thumbUrl(path, size)
			: fileUrl("textures", path)
		: null;

	if (!src || failed) {
		return (
			<div
				aria-hidden
				className={cn(
					"flex items-center justify-center text-muted-foreground/70",
					className,
				)}
				title={alt || undefined}
			>
				{name ? (
					<span className="select-none font-heading font-semibold text-[1.6em] tracking-tight">
						{initials(name)}
					</span>
				) : (
					<ImageOffIcon className="size-1/4 min-w-4 opacity-60" />
				)}
			</div>
		);
	}
	return (
		<div
			className={cn(
				"flex items-center justify-center overflow-hidden",
				className,
			)}
		>
			<img
				alt={alt}
				className={cn("size-full object-contain", imgClassName)}
				decoding="async"
				draggable={false}
				loading={eager ? "eager" : "lazy"}
				onError={() => setFailed(true)}
				src={src}
			/>
		</div>
	);
}

function initials(name: string): string {
	const words = name.trim().split(/\s+/).filter(Boolean);
	if (words.length === 0) return "?";
	if (words.length === 1) return [...(words[0] ?? "")].slice(0, 2).join("");
	return words
		.slice(0, 2)
		.map((w) => [...w][0])
		.join("");
}
