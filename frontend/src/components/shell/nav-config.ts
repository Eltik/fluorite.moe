import type { LinkOptions } from "@tanstack/react-router";
import {
	AudioLinesIcon,
	BookOpenIcon,
	BoxesIcon,
	FactoryIcon,
	HouseIcon,
	type LucideIcon,
	PackageIcon,
	SkullIcon,
	SwordIcon,
	UserRoundIcon,
	UsersIcon,
} from "lucide-react";
import type { CatalogKind } from "@/lib/api";
import { ROOT_META } from "@/lib/nav";

export interface NavItem {
	label: string;
	icon: LucideIcon;
	link: LinkOptions;
	/** Pathname used for active matching. */
	href: string;
	/** Shows the entity count from `/api/catalog`. */
	catalogKind?: CatalogKind;
	/** Rendered as the primary destination (Studio). */
	prominent?: boolean;
	/** One-line description used by the command palette and home page. */
	description?: string;
}

export interface NavSection {
	id: string;
	label?: string;
	/** Pathname prefix that belongs to this section (breadcrumb root). */
	href?: string;
	/** Breadcrumb link target for the section label. */
	link?: LinkOptions;
	/** Folded by default unless the current page lives inside it. */
	collapsible?: boolean;
	items: NavItem[];
}

const catalogItem = (
	kind: CatalogKind,
	label: string,
	icon: LucideIcon,
): NavItem => ({
	label,
	icon,
	link: { to: "/catalog/$kind", params: { kind } },
	href: `/catalog/${kind}`,
	catalogKind: kind,
});

export const NAV: NavSection[] = [
	{
		id: "main",
		items: [
			{ label: "Home", icon: HouseIcon, link: { to: "/" }, href: "/" },
			{
				label: "Studio",
				icon: BoxesIcon,
				link: { to: "/studio" },
				href: "/studio",
				prominent: true,
				description: "3D viewer for operators, enemies and weapons",
			},
		],
	},
	{
		id: "catalog",
		label: "Catalog",
		href: "/catalog",
		link: { to: "/catalog" },
		items: [
			catalogItem("operators", "Operators", UsersIcon),
			catalogItem("weapons", "Weapons", SwordIcon),
			catalogItem("enemies", "Enemies", SkullIcon),
			catalogItem("items", "Items", PackageIcon),
			catalogItem("npcs", "NPCs", UserRoundIcon),
			catalogItem("facilities", "Facilities", FactoryIcon),
		],
	},
	{
		id: "media",
		items: [
			{
				label: "Story",
				icon: BookOpenIcon,
				link: { to: "/story" },
				href: "/story",
				description: "Every mission dialog, read as a chat transcript",
			},
			{
				label: "Audio",
				icon: AudioLinesIcon,
				link: { to: "/audio" },
				href: "/audio",
				description: "Voice lines, music and sound effects",
			},
		],
	},
	{
		id: "files",
		label: "Game files",
		collapsible: true,
		items: [
			{
				label: ROOT_META.textures.label,
				icon: ROOT_META.textures.icon,
				link: { to: "/textures" },
				href: "/textures",
			},
			{
				label: ROOT_META.meshes.label,
				icon: ROOT_META.meshes.icon,
				link: { to: "/browse/$root", params: { root: "meshes" } },
				href: "/browse/meshes",
			},
			{
				label: ROOT_META.tables.label,
				icon: ROOT_META.tables.icon,
				link: { to: "/tables" },
				href: "/tables",
			},
			{
				label: ROOT_META.jsondata.label,
				icon: ROOT_META.jsondata.icon,
				link: { to: "/browse/$root", params: { root: "jsondata" } },
				href: "/browse/jsondata",
			},
			{
				label: ROOT_META.lua.label,
				icon: ROOT_META.lua.icon,
				link: { to: "/browse/$root", params: { root: "lua" } },
				href: "/browse/lua",
			},
			{
				label: ROOT_META.gamedata.label,
				icon: ROOT_META.gamedata.icon,
				link: { to: "/browse/$root", params: { root: "gamedata" } },
				href: "/browse/gamedata",
			},
		],
	},
];

export function isActive(pathname: string, href: string): boolean {
	if (href === "/") return pathname === "/";
	return pathname === href || pathname.startsWith(`${href}/`);
}

/** The nav section a pathname belongs to (drives the breadcrumb root). */
export function sectionFor(pathname: string): NavSection | undefined {
	return NAV.find(
		(section) =>
			(section.href && isActive(pathname, section.href)) ||
			section.items.some(
				(item) => item.href !== "/" && isActive(pathname, item.href),
			),
	);
}
