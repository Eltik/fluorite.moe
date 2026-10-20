import "@tanstack/react-router";

/** A label, or a function of the route params (and its loader data, when any). */
export type Crumb =
	| string
	| ((params: Record<string, string>, loaderData?: unknown) => string);

declare module "@tanstack/react-router" {
	interface StaticDataRouteOption {
		/** Sidebar section shown before the crumbs (e.g. "Data"). */
		section?: Crumb;
		/** Label of this route in the top-bar breadcrumb. */
		crumb?: Crumb;
		/** Render without the app shell (sidebar / top bar), e.g. the Studio. */
		fullBleed?: boolean;
	}
}

export function crumbLabel(
	crumb: Crumb,
	params: Record<string, string>,
	loaderData?: unknown,
): string {
	return typeof crumb === "function" ? crumb(params, loaderData) : crumb;
}

export function pageTitle(...parts: string[]): string {
	return [...parts.filter(Boolean), "fluorite"].join(" · ");
}
