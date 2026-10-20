import { useQuery } from "@tanstack/react-query";
import { Link, useLocation } from "@tanstack/react-router";
import { SearchIcon } from "lucide-react";
import type * as React from "react";
import { useCommandPalette } from "@/components/shell/command-palette";
import { LogoMark, Wordmark } from "@/components/shell/logo";
import { isActive, NAV } from "@/components/shell/nav-config";
import { Badge } from "@/components/ui/badge";
import { Kbd } from "@/components/ui/kbd";
import {
	Sidebar,
	SidebarContent,
	SidebarFooter,
	SidebarGroup,
	SidebarGroupContent,
	SidebarGroupLabel,
	SidebarHeader,
	SidebarMenu,
	SidebarMenuBadge,
	SidebarMenuButton,
	SidebarMenuItem,
	SidebarRail,
	useSidebar,
} from "@/components/ui/sidebar";
import { formatNumber } from "@/lib/format";
import { statusQuery } from "@/lib/queries";

function compact(n: number): string {
	if (n >= 10_000) return `${Math.round(n / 1000)}k`;
	if (n >= 1_000) return `${(n / 1000).toFixed(1)}k`;
	return String(n);
}

export function AppSidebar(): React.ReactElement {
	const pathname = useLocation({ select: (l) => l.pathname });
	const { isMobile, setOpenMobile } = useSidebar();
	const { data: status } = useQuery(statusQuery());
	const palette = useCommandPalette();

	const closeOnMobile = () => {
		if (isMobile) setOpenMobile(false);
	};

	return (
		<Sidebar collapsible="icon" variant="sidebar">
			<SidebarHeader className="gap-3 px-3 pt-3">
				<Link
					aria-label="fluorite home"
					className="flex h-8 items-center gap-2 rounded-md px-1 outline-none focus-visible:ring-2 focus-visible:ring-ring group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0"
					onClick={closeOnMobile}
					to="/"
				>
					<LogoMark className="size-5 shrink-0" />
					<Wordmark className="group-data-[collapsible=icon]:hidden" />
				</Link>
				<SidebarMenu className="group-data-[collapsible=icon]:hidden">
					<SidebarMenuItem>
						<SidebarMenuButton
							className="border border-sidebar-border bg-background text-muted-foreground shadow-xs/5 hover:text-foreground dark:bg-input/32"
							onClick={() => {
								closeOnMobile();
								palette.setOpen(true);
							}}
						>
							<SearchIcon />
							<span className="flex-1">Search…</span>
							<Kbd className="bg-transparent">⌘K</Kbd>
						</SidebarMenuButton>
					</SidebarMenuItem>
				</SidebarMenu>
			</SidebarHeader>
			<SidebarContent>
				{NAV.map((section, i) => (
					<SidebarGroup key={section.label ?? i}>
						{section.label && (
							<SidebarGroupLabel>{section.label}</SidebarGroupLabel>
						)}
						<SidebarGroupContent>
							<SidebarMenu>
								{section.items.map((item) => {
									const count =
										item.count && status
											? status.counts[item.count]
											: undefined;
									const Icon = item.icon;
									return (
										<SidebarMenuItem key={item.href}>
											<SidebarMenuButton
												isActive={isActive(pathname, item.href)}
												render={
													<Link
														{...item.link}
														aria-current={
															isActive(pathname, item.href) ? "page" : undefined
														}
														onClick={closeOnMobile}
													/>
												}
												tooltip={item.label}
											>
												<Icon />
												<span>{item.label}</span>
											</SidebarMenuButton>
											{count !== undefined && (
												<SidebarMenuBadge
													className="font-normal text-muted-foreground tabular-nums"
													title={`${formatNumber(count)} files`}
												>
													{compact(count)}
												</SidebarMenuBadge>
											)}
											{item.badge && (
												<SidebarMenuBadge>
													<Badge
														className="font-normal"
														size="sm"
														variant="outline"
													>
														{item.badge}
													</Badge>
												</SidebarMenuBadge>
											)}
										</SidebarMenuItem>
									);
								})}
							</SidebarMenu>
						</SidebarGroupContent>
					</SidebarGroup>
				))}
			</SidebarContent>
			<SidebarFooter className="px-3 pb-3 group-data-[collapsible=icon]:hidden">
				<p className="text-pretty text-muted-foreground text-xs leading-relaxed">
					Unofficial fan project. Game data © its respective owners.
				</p>
			</SidebarFooter>
			<SidebarRail />
		</Sidebar>
	);
}
