import type * as React from "react";
import type { CatalogTag } from "@/lib/api";
import { fileUrl } from "@/lib/api";
import { ELEMENT_COLORS, TAG_KIND_LABELS, tagIcon } from "@/lib/catalog";
import { cn } from "@/lib/utils";

/** A class / element / weapon glyph, tinted with `currentColor`. */
export function TagGlyph({
	tag,
	className,
}: {
	tag: CatalogTag;
	className?: string;
}): React.ReactElement | null {
	const icon = tagIcon(tag);
	if (!icon) return null;
	// Physical's grey would vanish on the card; it reads as the neutral glyph.
	const color =
		tag.kind === "element" && tag.code !== "Physical"
			? ELEMENT_COLORS[tag.code]
			: undefined;
	const image = `url("${fileUrl("textures", icon)}")`;
	// Weapon-type sprites are stencils (a filled tile with the glyph cut out);
	// subtracting them from a solid layer gives the glyph back.
	const stencil = tag.kind === "weaponType";
	const mask = stencil ? `linear-gradient(#000 0 0), ${image}` : image;
	const label = `${TAG_KIND_LABELS[tag.kind] ?? tag.kind}: ${tag.name}`;
	return (
		<span
			aria-label={label}
			className={cn(
				"inline-block size-4.5 shrink-0 bg-current",
				color
					? "[color:oklch(from_var(--glyph)_min(l,0.6)_c_h)] dark:[color:var(--glyph)]"
					: "text-foreground/60",
				className,
			)}
			role="img"
			style={
				{
					"--glyph": color,
					maskImage: mask,
					WebkitMaskImage: mask,
					// Stencils: oversize so the tile's soft edges fall outside the box.
					maskSize: stencil ? "auto, 142%" : "contain",
					WebkitMaskSize: stencil ? "auto, 142%" : "contain",
					maskRepeat: "no-repeat",
					WebkitMaskRepeat: "no-repeat",
					maskPosition: "center",
					WebkitMaskPosition: "center",
					maskComposite: stencil ? "exclude" : undefined,
					WebkitMaskComposite: stencil ? "xor" : undefined,
				} as React.CSSProperties
			}
			title={label}
		/>
	);
}
