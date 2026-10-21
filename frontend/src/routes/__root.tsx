import type { QueryClient } from "@tanstack/react-query";
import {
	createRootRouteWithContext,
	HeadContent,
	Outlet,
	Scripts,
	useMatches,
} from "@tanstack/react-router";
import * as React from "react";
import { AppShell } from "@/components/shell/app-shell";
import { VibecodedNotice } from "@/components/shell/vibecoded-notice";
import { AnchoredToastProvider, ToastProvider } from "@/components/ui/toast";
import { apiConfigScript } from "@/lib/api";
import { PREHYDRATION_SCRIPT } from "@/lib/settings";
import appCss from "../styles.css?url";

export interface RouterContext {
	queryClient: QueryClient;
}

/** Devtools are dev-only; the dynamic import keeps them out of production. */
const Devtools = import.meta.env.DEV
	? React.lazy(() => import("@/components/devtools"))
	: () => null;

export const Route = createRootRouteWithContext<RouterContext>()({
	head: () => ({
		meta: [
			{ charSet: "utf-8" },
			{ name: "viewport", content: "width=device-width, initial-scale=1" },
			{ title: "fluorite" },
			{
				name: "description",
				content:
					"fluorite: an Arknights: Endfield data studio for browsing tables, textures, models and scripts.",
			},
			{ name: "color-scheme", content: "light dark" },
		],
		links: [
			{ rel: "stylesheet", href: appCss },
			{ rel: "icon", type: "image/svg+xml", href: "/favicon.svg" },
		],
	}),
	shellComponent: RootDocument,
	component: RootLayout,
});

function RootDocument({
	children,
}: {
	children: React.ReactNode;
}): React.ReactElement {
	return (
		<html lang="en" suppressHydrationWarning>
			<head>
				{/* Runtime API location (server env), read by `lib/api.ts` in the browser. */}
				<script
					// biome-ignore lint/security/noDangerouslySetInnerHtml: JSON-encoded config value, `<` escaped
					dangerouslySetInnerHTML={{ __html: apiConfigScript() }}
				/>
				{/* Applies theme + density before first paint (no flash). */}
				<script
					// biome-ignore lint/security/noDangerouslySetInnerHtml: static, first-party pre-hydration script
					dangerouslySetInnerHTML={{ __html: PREHYDRATION_SCRIPT }}
				/>
				<HeadContent />
			</head>
			<body className="relative">
				<div className="isolate">
					<ToastProvider position="bottom-right">
						<AnchoredToastProvider>{children}</AnchoredToastProvider>
					</ToastProvider>
				</div>
				<React.Suspense>
					<Devtools />
				</React.Suspense>
				<Scripts />
			</body>
		</html>
	);
}

function RootLayout(): React.ReactElement {
	const fullBleed = useMatches({
		select: (matches) => matches.some((m) => m.staticData?.fullBleed),
	});
	return (
		<>
			{fullBleed ? (
				<Outlet />
			) : (
				<AppShell>
					<Outlet />
				</AppShell>
			)}
			<VibecodedNotice />
		</>
	);
}
