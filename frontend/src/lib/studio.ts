/**
 * Studio routing helpers shared with the rest of the app. Kept free of three /
 * r3f imports so catalog pages can link into the Studio without pulling the
 * 3D bundle.
 */
import type { CatalogKind } from "@/lib/api";

export const STUDIO_KINDS = [
	"operators",
	"enemies",
	"weapons",
	"npcs",
] as const satisfies readonly CatalogKind[];
export type StudioKind = (typeof STUDIO_KINDS)[number];

export const STUDIO_KIND_LABELS: Record<StudioKind, string> = {
	operators: "Operators",
	enemies: "Enemies",
	weapons: "Weapons",
	npcs: "NPCs",
};

export const STUDIO_KIND_SINGULAR: Record<StudioKind, string> = {
	operators: "Operator",
	enemies: "Enemy",
	weapons: "Weapon",
	npcs: "NPC",
};

export function isStudioKind(value: string | undefined): value is StudioKind {
	return (
		value !== undefined && (STUDIO_KINDS as readonly string[]).includes(value)
	);
}

export interface StudioSearch {
	/** Background id (`stage-light`, `black`, …) or a `#rrggbb` colour. */
	bg?: string;
	/** Animation clip name to select once the model is loaded. */
	clip?: string;
}

/** Link options for opening an entity in the Studio: `<Link {...studioLink(k, id)} />`. */
export function studioLink(kind: StudioKind, id: string) {
	return {
		to: "/studio/$kind/$id",
		params: { kind, id },
	} as const;
}
