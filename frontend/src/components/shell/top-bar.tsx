import { useQuery } from "@tanstack/react-query";
import { Link, useMatches } from "@tanstack/react-router";
import {
	LanguagesIcon,
	MonitorIcon,
	MoonIcon,
	SearchIcon,
	SlidersHorizontalIcon,
	SunIcon,
} from "lucide-react";
import * as React from "react";
import { useCommandPalette } from "@/components/shell/command-palette";
import { StatusIndicator } from "@/components/shell/status-indicator";
import {
	Breadcrumb,
	BreadcrumbItem,
	BreadcrumbLink,
	BreadcrumbList,
	BreadcrumbPage,
	BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import {
	Menu,
	MenuGroup,
	MenuGroupLabel,
	MenuPopup,
	MenuRadioGroup,
	MenuRadioItem,
	MenuSeparator,
	MenuTrigger,
} from "@/components/ui/menu";
import {
	Select,
	SelectItem,
	SelectPopup,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { SidebarTrigger } from "@/components/ui/sidebar";
import { languagesQuery } from "@/lib/queries";
import { crumbLabel } from "@/lib/route-meta";
import {
	type Density,
	type ThemePreference,
	updateSettings,
	useSettings,
} from "@/lib/settings";

const LANGUAGE_NAMES: Record<string, string> = {
	EN: "English",
	CN: "简体中文",
	TC: "繁體中文",
	JP: "日本語",
	KR: "한국어",
	BR: "Português (BR)",
	DE: "Deutsch",
	FR: "Français",
	ID: "Bahasa Indonesia",
	IT: "Italiano",
	MX: "Español (MX)",
	RU: "Русский",
	TH: "ไทย",
	VN: "Tiếng Việt",
};

export function TopBar(): React.ReactElement {
	return (
		<header className="sticky top-0 z-20 flex h-12 shrink-0 items-center gap-2 border-b bg-background/80 px-3 backdrop-blur-md supports-[backdrop-filter]:bg-background/64 md:px-4">
			<SidebarTrigger className="-ms-1" />
			<Separator className="me-1 h-4" orientation="vertical" />
			<Crumbs />
			<div className="ms-auto flex items-center gap-1">
				<SearchTrigger />
				<StatusIndicator />
				<LanguageSelect />
				<DisplayMenu />
			</div>
		</header>
	);
}

function Crumbs(): React.ReactElement {
	const matches = useMatches();
	const items = React.useMemo(() => {
		const out: { label: string; to?: string }[] = [];
		// The deepest route that declares a section wins (e.g. "Data").
		const sectionMatch = [...matches]
			.reverse()
			.find((m) => m.staticData.section);
		const section = sectionMatch?.staticData.section;
		if (section && sectionMatch) {
			out.push({
				label: crumbLabel(
					section,
					sectionMatch.params as Record<string, string>,
				),
			});
		}
		for (const match of matches) {
			const crumb = match.staticData.crumb;
			if (!crumb) continue;
			const label = crumbLabel(crumb, match.params as Record<string, string>);
			if (out[out.length - 1]?.label === label) continue;
			out.push({ label, to: match.pathname });
		}
		return out;
	}, [matches]);

	return (
		<Breadcrumb className="min-w-0">
			<BreadcrumbList className="flex-nowrap sm:gap-2">
				{items.map((item, i) => {
					const isLast = i === items.length - 1;
					return (
						<React.Fragment key={item.to ?? item.label}>
							{i > 0 && <BreadcrumbSeparator className="max-sm:hidden" />}
							<BreadcrumbItem className={isLast ? "min-w-0" : "max-sm:hidden"}>
								{isLast || !item.to ? (
									<BreadcrumbPage className="truncate">
										{item.label}
									</BreadcrumbPage>
								) : (
									<BreadcrumbLink render={<Link to={item.to} />}>
										{item.label}
									</BreadcrumbLink>
								)}
							</BreadcrumbItem>
						</React.Fragment>
					);
				})}
			</BreadcrumbList>
		</Breadcrumb>
	);
}

function SearchTrigger(): React.ReactElement {
	const { setOpen } = useCommandPalette();
	return (
		<>
			<Button
				className="hidden w-56 justify-start text-muted-foreground lg:inline-flex"
				onClick={() => setOpen(true)}
				variant="outline"
			>
				<SearchIcon />
				<span className="flex-1 text-start font-normal">Search files…</span>
				<Kbd>⌘K</Kbd>
			</Button>
			<Button
				aria-label="Search"
				className="lg:hidden"
				onClick={() => setOpen(true)}
				size="icon"
				variant="ghost"
			>
				<SearchIcon />
			</Button>
		</>
	);
}

function LanguageSelect(): React.ReactElement {
	const { textLanguage } = useSettings();
	const { data: languages } = useQuery(languagesQuery());
	const options = languages ?? Object.keys(LANGUAGE_NAMES);
	const items = options.map((code) => ({
		value: code,
		label: LANGUAGE_NAMES[code] ?? code,
	}));
	return (
		<Select
			items={items}
			onValueChange={(value) => {
				if (typeof value === "string") updateSettings({ textLanguage: value });
			}}
			value={textLanguage}
		>
			<SelectTrigger
				aria-label="Game text language"
				className="w-auto min-w-0 gap-1.5 border-transparent bg-transparent shadow-none before:hidden dark:bg-transparent"
				size="sm"
				title="Game text language"
			>
				<LanguagesIcon className="size-4 opacity-80" />
				<SelectValue>{(value: string) => value}</SelectValue>
			</SelectTrigger>
			<SelectPopup align="end">
				{items.map((item) => (
					<SelectItem key={item.value} value={item.value}>
						<span className="flex w-full items-center justify-between gap-4">
							{item.label}
							<span className="text-muted-foreground text-xs">
								{item.value}
							</span>
						</span>
					</SelectItem>
				))}
			</SelectPopup>
		</Select>
	);
}

const THEME_ICON: Record<ThemePreference, typeof SunIcon> = {
	light: SunIcon,
	dark: MoonIcon,
	system: MonitorIcon,
};

function DisplayMenu(): React.ReactElement {
	const { theme, density } = useSettings();
	const Icon = THEME_ICON[theme] ?? SlidersHorizontalIcon;
	return (
		<Menu>
			<MenuTrigger
				render={
					<Button aria-label="Display settings" size="icon" variant="ghost" />
				}
			>
				<Icon />
			</MenuTrigger>
			<MenuPopup align="end" className="min-w-44">
				<MenuGroup>
					<MenuGroupLabel>Theme</MenuGroupLabel>
					<MenuRadioGroup
						onValueChange={(value) =>
							updateSettings({ theme: value as ThemePreference })
						}
						value={theme}
					>
						<MenuRadioItem value="light">
							<SunIcon />
							Light
						</MenuRadioItem>
						<MenuRadioItem value="dark">
							<MoonIcon />
							Dark
						</MenuRadioItem>
						<MenuRadioItem value="system">
							<MonitorIcon />
							System
						</MenuRadioItem>
					</MenuRadioGroup>
				</MenuGroup>
				<MenuSeparator />
				<MenuGroup>
					<MenuGroupLabel>Density</MenuGroupLabel>
					<MenuRadioGroup
						onValueChange={(value) =>
							updateSettings({ density: value as Density })
						}
						value={density}
					>
						<MenuRadioItem value="comfortable">Comfortable</MenuRadioItem>
						<MenuRadioItem value="compact">Compact</MenuRadioItem>
					</MenuRadioGroup>
				</MenuGroup>
			</MenuPopup>
		</Menu>
	);
}
