import type { LinkOptions } from "@tanstack/react-router";
import {
	BoxesIcon,
	HouseIcon,
	type LucideIcon,
	PackageIcon,
	SkullIcon,
	SwordIcon,
	UsersIcon,
} from "lucide-react";
import type { StatusCounts } from "@/lib/api";
import { ROOT_META } from "@/lib/nav";

export interface NavItem {
	label: string;
	icon: LucideIcon;
	link: LinkOptions;
	/** Pathname used for active matching. */
	href: string;
	count?: keyof StatusCounts;
	badge?: string;
}

export interface NavSection {
	label?: string;
	items: NavItem[];
}

export const NAV: NavSection[] = [
	{
		items: [
			{ label: "Home", icon: HouseIcon, link: { to: "/" }, href: "/" },
			{
				label: "Studio",
				icon: BoxesIcon,
				link: { to: "/studio" },
				href: "/studio",
				badge: "Soon",
			},
		],
	},
	{
		label: "Catalog",
		items: [
			{
				label: "Operators",
				icon: UsersIcon,
				link: { to: "/catalog/$kind", params: { kind: "operators" } },
				href: "/catalog/operators",
			},
			{
				label: "Enemies",
				icon: SkullIcon,
				link: { to: "/catalog/$kind", params: { kind: "enemies" } },
				href: "/catalog/enemies",
			},
			{
				label: "Weapons",
				icon: SwordIcon,
				link: { to: "/catalog/$kind", params: { kind: "weapons" } },
				href: "/catalog/weapons",
			},
			{
				label: "Items",
				icon: PackageIcon,
				link: { to: "/catalog/$kind", params: { kind: "items" } },
				href: "/catalog/items",
			},
		],
	},
	{
		label: "Assets",
		items: [
			{
				label: ROOT_META.textures.label,
				icon: ROOT_META.textures.icon,
				link: { to: "/textures" },
				href: "/textures",
				count: "textures",
			},
			{
				label: ROOT_META.meshes.label,
				icon: ROOT_META.meshes.icon,
				link: { to: "/browse/$root", params: { root: "meshes" } },
				href: "/browse/meshes",
				count: "meshes",
			},
			{
				label: ROOT_META.audio.label,
				icon: ROOT_META.audio.icon,
				link: { to: "/browse/$root", params: { root: "audio" } },
				href: "/browse/audio",
			},
		],
	},
	{
		label: "Data",
		items: [
			{
				label: ROOT_META.tables.label,
				icon: ROOT_META.tables.icon,
				link: { to: "/tables" },
				href: "/tables",
				count: "tables",
			},
			{
				label: ROOT_META.jsondata.label,
				icon: ROOT_META.jsondata.icon,
				link: { to: "/browse/$root", params: { root: "jsondata" } },
				href: "/browse/jsondata",
				count: "jsondata",
			},
			{
				label: ROOT_META.lua.label,
				icon: ROOT_META.lua.icon,
				link: { to: "/browse/$root", params: { root: "lua" } },
				href: "/browse/lua",
				count: "lua",
			},
			{
				label: ROOT_META.gamedata.label,
				icon: ROOT_META.gamedata.icon,
				link: { to: "/browse/$root", params: { root: "gamedata" } },
				href: "/browse/gamedata",
				count: "gamedata",
			},
		],
	},
];

export function isActive(pathname: string, href: string): boolean {
	if (href === "/") return pathname === "/";
	return pathname === href || pathname.startsWith(`${href}/`);
}
