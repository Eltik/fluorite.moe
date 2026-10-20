import * as React from "react";
import { AppSidebar } from "@/components/shell/app-sidebar";
import { CommandPaletteProvider } from "@/components/shell/command-palette";
import { TopBar } from "@/components/shell/top-bar";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";

function readSidebarCookie(): boolean | null {
	const match = document.cookie.match(/(?:^|;\s*)sidebar_state=(true|false)/);
	return match ? match[1] === "true" : null;
}

export function AppShell({
	children,
}: {
	children: React.ReactNode;
}): React.ReactElement {
	const [open, setOpen] = React.useState(true);

	React.useEffect(() => {
		const saved = readSidebarCookie();
		if (saved !== null) setOpen(saved);
	}, []);

	return (
		<CommandPaletteProvider>
			<a
				className="sr-only z-50 rounded-md bg-background px-3 py-2 text-sm shadow focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
				href="#content"
			>
				Skip to content
			</a>
			<SidebarProvider onOpenChange={setOpen} open={open}>
				<AppSidebar />
				<SidebarInset className="h-svh min-w-0 overflow-hidden">
					<TopBar />
					<div
						className="relative flex min-h-0 flex-1 flex-col overflow-y-auto"
						id="content"
						tabIndex={-1}
					>
						{children}
					</div>
				</SidebarInset>
			</SidebarProvider>
		</CommandPaletteProvider>
	);
}
