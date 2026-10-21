/**
 * Character-screen data for a rigged model: the game camera, the menu state
 * machine, the default weapon and the deco props, fetched as JSON next to
 * the model. The prop GLBs are handled by `props-rig.ts`; the decisions
 * (what shows when) live in `lib/props` and `lib/menu`.
 */
import type { StudioModelSource } from "@/components/studio/catalog-source";
import type { LoadedModel } from "@/components/studio/load-model";
import { modelFileUrl } from "@/lib/api";
import { dirname } from "@/lib/format";
import type { GameCamera } from "@/lib/framing";
import { type MenuMachine, parseMenu } from "@/lib/menu";
import {
	type DecoIndex,
	parseDecoIndex,
	parseWeaponInfo,
	type WeaponInfo,
} from "@/lib/props";

export interface CharScreen {
	camera: GameCamera | null;
	menu: MenuMachine | null;
	weapon: WeaponInfo | null;
	deco: DecoIndex | null;
	/** Folder of the deco index, for its GLBs. */
	decoDir: string | null;
}

async function fetchJson(
	path: string | undefined,
	signal: AbortSignal,
): Promise<unknown> {
	if (!path) return null;
	try {
		const res = await fetch(modelFileUrl(path), { signal });
		if (!res.ok) return null;
		return await res.json();
	} catch (error) {
		if (signal.aborted) throw error;
		return null;
	}
}

/** The JSON side of the character screen; missing files become nulls. */
export async function loadCharScreen(
	source: StudioModelSource,
	signal: AbortSignal,
): Promise<CharScreen> {
	const [menu, weapon, deco] = await Promise.all([
		fetchJson(source.menu, signal),
		fetchJson(source.weapon, signal),
		fetchJson(source.deco, signal),
	]);
	return {
		camera: source.camera ?? null,
		menu: parseMenu(menu),
		weapon: parseWeaponInfo(weapon),
		deco: parseDecoIndex(deco),
		decoDir: source.deco ? dirname(source.deco) : null,
	};
}

/** Whether a model has anything a `PropsRig` could show. */
export function hasWeapon(model: LoadedModel | null): boolean {
	return Boolean(model?.charscreen?.weapon?.glb);
}

export function hasProps(model: LoadedModel | null): boolean {
	return Boolean(model?.charscreen?.deco?.items.some((i) => i.file));
}
