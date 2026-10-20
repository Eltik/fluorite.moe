import "@tanstack/react-router";

export type Crumb = string | ((params: Record<string, string>) => string);

declare module "@tanstack/react-router" {
	interface StaticDataRouteOption {
		/** Sidebar section shown before the crumbs (e.g. "Data"). */
		section?: Crumb;
		/** Label of this route in the top-bar breadcrumb. */
		crumb?: Crumb;
	}
}

export function crumbLabel(
	crumb: Crumb,
	params: Record<string, string>,
): string {
	return typeof crumb === "function" ? crumb(params) : crumb;
}

export function pageTitle(...parts: string[]): string {
	return [...parts.filter(Boolean), "fluorite"].join(" · ");
}
