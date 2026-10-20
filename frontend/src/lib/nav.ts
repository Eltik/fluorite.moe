import {
	AudioLinesIcon,
	BoxIcon,
	BracesIcon,
	DatabaseIcon,
	FileCodeIcon,
	ImageIcon,
	type LucideIcon,
	TableIcon,
} from "lucide-react";
import type { Root } from "@/lib/api";
import { basename, dirname } from "@/lib/format";

export interface RootMeta {
	label: string;
	icon: LucideIcon;
	description: string;
}

export const ROOT_META: Record<Root, RootMeta> = {
	textures: {
		label: "Textures",
		icon: ImageIcon,
		description: "PNG textures in Unity container layout",
	},
	meshes: {
		label: "Models",
		icon: BoxIcon,
		description: "Static GLB meshes",
	},
	audio: {
		label: "Audio",
		icon: AudioLinesIcon,
		description: "Audio banks and packs",
	},
	tables: {
		label: "Tables",
		icon: TableIcon,
		description: "Decoded game tables",
	},
	jsondata: {
		label: "JsonData",
		icon: BracesIcon,
		description: "Decoded MemoryPack JSON",
	},
	lua: {
		label: "Lua",
		icon: FileCodeIcon,
		description: "Decrypted Lua scripts",
	},
	gamedata: {
		label: "Gamedata",
		icon: DatabaseIcon,
		description: "TextAsset and MonoBehaviour JSON",
	},
};

/** Where a search hit (or any file) opens. */
export type FileTarget =
	| { to: "/tables/$name"; params: { name: string } }
	| {
			to: "/browse/$root";
			params: { root: Root };
			search: { path: string; file: string };
	  };

export function fileTarget(root: Root, path: string): FileTarget {
	if (root === "tables" && path.endsWith(".json") && !path.includes("/")) {
		return {
			to: "/tables/$name",
			params: { name: basename(path).replace(/\.json$/, "") },
		};
	}
	return {
		to: "/browse/$root",
		params: { root },
		search: { path: dirname(path), file: path },
	};
}
