import { useQuery } from "@tanstack/react-query";
import { Link, useLocation } from "@tanstack/react-router";
import { ChevronRightIcon } from "lucide-react";
import * as React from "react";
import { LogoMark, Wordmark } from "@/components/shell/logo";
import {
	isActive,
	NAV,
	type NavItem,
	type NavSection,
} from "@/components/shell/nav-config";
import {
	Collapsible,
	CollapsiblePanel,
	CollapsibleTrigger,
} from "@/components/ui/collapsible";
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
import type { CatalogKind } from "@/lib/api";
import { formatCompact, formatNumber } from "@/lib/format";
import { catalogKindsQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";

export function AppSidebar(): React.ReactElement {
	const pathname = useLocation({ select: (l) => l.pathname });
	const { isMobile, setOpenMobile } = useSidebar();
	const { data: counts } = useQuery(catalogKindsQuery());

	const closeOnMobile = () => {
		if (isMobile) setOpenMobile(false);
	};

	return (
		<Sidebar collapsible="icon" variant="sidebar">
			<SidebarHeader className="px-3 pt-3 pb-0">
				<Link
					aria-label="fluorite home"
					className="flex h-8 items-center gap-2 rounded-md px-1 outline-none focus-visible:ring-2 focus-visible:ring-ring group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0"
					onClick={closeOnMobile}
					to="/"
				>
					<LogoMark className="size-5 shrink-0" />
					<Wordmark className="group-data-[collapsible=icon]:hidden" />
				</Link>
			</SidebarHeader>
			{/* SidebarContent is its own scroll area; the footer stays pinned. */}
			<SidebarContent className="gap-0">
				{NAV.map((section) =>
					section.collapsible ? (
						<CollapsibleSection
							counts={counts}
							key={section.id}
							onNavigate={closeOnMobile}
							pathname={pathname}
							section={section}
						/>
					) : (
						<SidebarGroup key={section.id}>
							{section.label && (
								<SidebarGroupLabel>{section.label}</SidebarGroupLabel>
							)}
							<SidebarGroupContent>
								<NavMenu
									counts={counts}
									items={section.items}
									onNavigate={closeOnMobile}
									pathname={pathname}
								/>
							</SidebarGroupContent>
						</SidebarGroup>
					),
				)}
			</SidebarContent>
			<SidebarFooter className="border-sidebar-border border-t px-3 py-2.5 group-data-[collapsible=icon]:hidden">
				<p className="text-pretty text-muted-foreground text-xs leading-relaxed">
					Unofficial fan project. Game data © its respective owners.
				</p>
			</SidebarFooter>
			<SidebarRail />
		</Sidebar>
	);
}

function CollapsibleSection({
	section,
	pathname,
	counts,
	onNavigate,
}: {
	section: NavSection;
	pathname: string;
	counts?: Partial<Record<CatalogKind, number>>;
	onNavigate: () => void;
}): React.ReactElement {
	const { state, isMobile } = useSidebar();
	const containsActive = section.items.some((item) =>
		isActive(pathname, item.href),
	);
	const [open, setOpen] = React.useState(containsActive);
	// Unfold when navigating into the section (e.g. from search or a link).
	React.useEffect(() => {
		if (containsActive) setOpen(true);
	}, [containsActive]);
	// The icon rail has no labels to click, so it always lists every item.
	const iconRail = state === "collapsed" && !isMobile;

	return (
		<Collapsible onOpenChange={setOpen} open={open || iconRail}>
			<SidebarGroup>
				<SidebarGroupLabel
					className="w-full cursor-pointer hover:bg-sidebar-accent hover:text-sidebar-accent-foreground data-panel-open:[&>svg]:rotate-90"
					render={<CollapsibleTrigger />}
				>
					{section.label}
					<ChevronRightIcon className="ms-auto transition-transform duration-200" />
				</SidebarGroupLabel>
				<CollapsiblePanel>
					<SidebarGroupContent>
						<NavMenu
							counts={counts}
							items={section.items}
							onNavigate={onNavigate}
							pathname={pathname}
						/>
					</SidebarGroupContent>
				</CollapsiblePanel>
			</SidebarGroup>
		</Collapsible>
	);
}

function NavMenu({
	items,
	pathname,
	counts,
	onNavigate,
}: {
	items: NavItem[];
	pathname: string;
	counts?: Partial<Record<CatalogKind, number>>;
	onNavigate: () => void;
}): React.ReactElement {
	return (
		<SidebarMenu>
			{items.map((item) => {
				const active = isActive(pathname, item.href);
				const count = item.catalogKind ? counts?.[item.catalogKind] : undefined;
				const Icon = item.icon;
				return (
					<SidebarMenuItem key={item.href}>
						<SidebarMenuButton
							className={cn(
								item.prominent &&
									"font-medium text-sidebar-accent-foreground [&>svg]:text-brand",
							)}
							isActive={active}
							render={
								<Link
									{...item.link}
									aria-current={active ? "page" : undefined}
									onClick={onNavigate}
								/>
							}
							tooltip={item.label}
						>
							{item.prominent ? (
								<span className="-ms-0.5 flex size-5 shrink-0 items-center justify-center rounded-md bg-brand/12 text-brand group-data-[collapsible=icon]:ms-0 group-data-[collapsible=icon]:bg-transparent [&>svg]:size-3.5 group-data-[collapsible=icon]:[&>svg]:size-4">
									<Icon />
								</span>
							) : (
								<Icon />
							)}
							<span>{item.label}</span>
						</SidebarMenuButton>
						{count !== undefined && (
							<SidebarMenuBadge
								className="font-normal text-muted-foreground tabular-nums"
								title={`${formatNumber(count)} ${item.label.toLowerCase()}`}
							>
								{formatCompact(count)}
							</SidebarMenuBadge>
						)}
					</SidebarMenuItem>
				);
			})}
		</SidebarMenu>
	);
}
