import * as React from "react";
import { EntityImage } from "@/components/catalog/entity-image";
import { RarityStars, rarityStyle } from "@/components/catalog/rarity";
import { Tabs, TabsList, TabsPanel, TabsTab } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { type CatalogKind, fileUrl } from "@/lib/api";
import type { Gender } from "@/lib/catalog";
import { cn } from "@/lib/utils";

/** What each kind's detail body receives from the route. */
export interface DetailProps<T> {
	data: T;
	tab: string | undefined;
	onTabChange: (tab: string) => void;
	gender: Gender;
	onGenderChange: (gender: Gender) => void;
	/** Whether this entry's text differs by Endministrator gender. */
	gendered?: boolean;
}

export function DetailPage({
	children,
}: {
	children: React.ReactNode;
}): React.ReactElement {
	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-5 p-4 md:p-6 lg:p-8">
			{children}
		</div>
	);
}

/**
 * Hero header. `art` is a large illustration shown on the right (faded into
 * the card); `icon` the square thumbnail on the left.
 */
export function DetailHero({
	name,
	subtitle,
	icon,
	art,
	artClassName,
	rarity,
	chips,
	actions,
	children,
}: {
	/** The entry's kind (the top bar shows the breadcrumb trail). */
	kind: CatalogKind;
	name: string;
	subtitle?: React.ReactNode;
	icon?: string | null;
	art?: string | null;
	artClassName?: string;
	rarity?: number | null;
	chips?: React.ReactNode;
	actions?: React.ReactNode;
	children?: React.ReactNode;
}): React.ReactElement {
	const [artFailed, setArtFailed] = React.useState(false);
	// biome-ignore lint/correctness/useExhaustiveDependencies: reset when the art changes
	React.useEffect(() => setArtFailed(false), [art]);
	const showArt = art && !artFailed;
	return (
		<header
			className="relative isolate overflow-hidden rounded-2xl border bg-card shadow-xs/5"
			style={rarityStyle(rarity)}
		>
			{rarity ? (
				<div
					aria-hidden
					className="-z-10 absolute inset-0 opacity-70"
					style={{
						backgroundImage:
							"radial-gradient(80% 120% at 85% 100%, color-mix(in oklab, var(--rarity) 22%, transparent), transparent 70%)",
					}}
				/>
			) : null}
			{showArt && (
				<img
					alt=""
					className={cn(
						"-z-10 pointer-events-none absolute inset-y-0 right-0 h-full w-[70%] object-cover object-[50%_24%] opacity-35 [mask-image:linear-gradient(to_left,black_45%,transparent)] sm:w-[58%] sm:opacity-95",
						artClassName,
					)}
					decoding="async"
					onError={() => setArtFailed(true)}
					src={fileUrl("textures", art)}
				/>
			)}
			{rarity ? (
				<span
					aria-hidden
					className="absolute inset-x-0 top-0 h-0.75 bg-(--rarity)"
				/>
			) : null}
			<div className="flex min-h-48 flex-col gap-4 p-4 sm:min-h-64 sm:p-6">
				<div className="flex flex-1 items-end gap-4">
					<EntityImage
						alt=""
						className="size-20 shrink-0 overflow-hidden rounded-xl border bg-background/70 text-xl backdrop-blur-sm sm:size-24"
						eager
						name={name}
						path={icon}
						size={256}
					/>
					<div className="flex min-w-0 flex-col gap-1.5 sm:max-w-[60%]">
						<RarityStars rarity={rarity} size="md" />
						<h1 className="text-balance font-heading font-semibold text-2xl tracking-tight sm:text-3xl">
							{name}
						</h1>
						{subtitle && (
							<div className="text-muted-foreground text-sm">{subtitle}</div>
						)}
					</div>
				</div>
				{(chips || actions) && (
					<div className="flex flex-wrap items-center justify-between gap-3">
						<div className="flex flex-wrap items-center gap-1.5">{chips}</div>
						{actions && (
							<div className="flex flex-wrap items-center gap-2 *:bg-background/72 *:backdrop-blur-sm *:rounded-lg">
								{actions}
							</div>
						)}
					</div>
				)}
				{children}
			</div>
		</header>
	);
}

/** A chip with an optional game icon, for profession / element / type. */
export function InfoChip({
	icon,
	children,
	color,
	className,
}: {
	icon?: string | null;
	children: React.ReactNode;
	color?: string | null;
	className?: string;
}): React.ReactElement {
	return (
		<span
			className={cn(
				"inline-flex h-7 items-center gap-1.5 rounded-full border bg-background/72 px-2.5 font-medium text-xs backdrop-blur-sm",
				color &&
					"border-[color-mix(in_oklab,var(--chip)_40%,transparent)] [color:oklch(from_var(--chip)_min(l,0.5)_c_h)] dark:[color:var(--chip)]",
				className,
			)}
			style={color ? ({ "--chip": color } as React.CSSProperties) : undefined}
		>
			{icon && (
				<img
					alt=""
					className="-ms-1 size-4.5 rounded-full bg-neutral-800/90 object-contain p-px dark:bg-transparent"
					src={`${fileUrl("textures", icon)}`}
				/>
			)}
			{children}
		</span>
	);
}

export interface DetailTab {
	value: string;
	label: string;
	count?: number;
	content: () => React.ReactNode;
}

/** URL-driven tabs (`?tab=`). Panels mount only while active. */
export function DetailTabs({
	tabs,
	tab,
	onTabChange,
}: {
	tabs: DetailTab[];
	tab: string | undefined;
	onTabChange: (tab: string) => void;
}): React.ReactElement {
	const active = tabs.some((t) => t.value === tab)
		? (tab as string)
		: tabs[0]?.value;
	return (
		<Tabs
			className="gap-4"
			onValueChange={(value) => onTabChange(String(value))}
			value={active}
		>
			<div className="-mx-4 overflow-x-auto px-4 [scrollbar-width:none] md:mx-0 md:px-0">
				<TabsList
					variant="underline"
					className="w-max min-w-full justify-start border-b"
				>
					{tabs.map((t) => (
						<TabsTab key={t.value} value={t.value}>
							{t.label}
							{t.count !== undefined && (
								<span className="text-muted-foreground text-xs tabular-nums">
									{t.count}
								</span>
							)}
						</TabsTab>
					))}
				</TabsList>
			</div>
			{tabs.map((t) => (
				<TabsPanel key={t.value} value={t.value}>
					{t.value === active ? t.content() : null}
				</TabsPanel>
			))}
		</Tabs>
	);
}

export function Section({
	title,
	description,
	action,
	children,
	className,
}: {
	title: React.ReactNode;
	description?: React.ReactNode;
	action?: React.ReactNode;
	children: React.ReactNode;
	className?: string;
}): React.ReactElement {
	return (
		<section className={cn("flex flex-col gap-3", className)}>
			<div className="flex flex-wrap items-end justify-between gap-2">
				<div className="min-w-0">
					<h2 className="font-heading font-semibold text-base tracking-tight">
						{title}
					</h2>
					{description && (
						<p className="text-muted-foreground text-sm">{description}</p>
					)}
				</div>
				{action}
			</div>
			{children}
		</section>
	);
}

/** Bordered surface used for detail blocks. */
export function Panel({
	children,
	className,
}: {
	children: React.ReactNode;
	className?: string;
}): React.ReactElement {
	return (
		<div className={cn("rounded-xl border bg-card p-4 shadow-xs/5", className)}>
			{children}
		</div>
	);
}

export function Facts({
	items,
	className,
}: {
	items: { label: string; value: React.ReactNode }[];
	className?: string;
}): React.ReactElement {
	const shown = items.filter(
		(i) => i.value !== null && i.value !== undefined && i.value !== "",
	);
	return (
		<dl
			className={cn(
				"grid grid-cols-[minmax(6rem,auto)_1fr] gap-x-4 gap-y-2 text-sm",
				className,
			)}
		>
			{shown.map((item) => (
				<React.Fragment key={item.label}>
					<dt className="text-muted-foreground">{item.label}</dt>
					<dd className="min-w-0 break-words">{item.value}</dd>
				</React.Fragment>
			))}
		</dl>
	);
}

export function GenderToggle({
	value,
	onChange,
	showLabel = false,
}: {
	value: Gender;
	onChange: (gender: Gender) => void;
	/** Prefix the ♀/♂ pair with a visible "Endministrator" label. */
	showLabel?: boolean;
}): React.ReactElement {
	const toggle = (
		<ToggleGroup
			aria-label="Endministrator gender for {F}/{M} text"
			onValueChange={(next) => {
				const v = next[0];
				if (v === "f" || v === "m") onChange(v);
			}}
			size="sm"
			title="Endministrator gender used for {F}/{M} text variants"
			value={[value]}
			variant="outline"
		>
			<ToggleGroupItem aria-label="Female Endministrator" value="f">
				♀
			</ToggleGroupItem>
			<ToggleGroupItem aria-label="Male Endministrator" value="m">
				♂
			</ToggleGroupItem>
		</ToggleGroup>
	);
	if (!showLabel) return toggle;
	return (
		<div className="inline-flex items-center gap-2 bg-transparent! backdrop-blur-none!">
			<span className="text-muted-foreground text-xs">Endministrator</span>
			{toggle}
		</div>
	);
}

/** Formats a stat value: integers grouped, fractions up to 3 decimals. */
export function formatStat(value: number, code?: string): string {
	if (
		code &&
		/(CriticalRate|DamageIncrease|Resistance)$/.test(code) &&
		Math.abs(value) <= 5
	) {
		return `${+(value * 100).toFixed(1)}%`;
	}
	if (Number.isInteger(value)) return value.toLocaleString();
	return (+value.toFixed(3)).toLocaleString(undefined, {
		maximumFractionDigits: 3,
	});
}

/** Long text clamped to a few lines with a "Read more" toggle. */
export function ClampedText({
	children,
	lines = 5,
}: {
	children: React.ReactNode;
	lines?: number;
}): React.ReactElement {
	const ref = React.useRef<HTMLDivElement>(null);
	const [open, setOpen] = React.useState(false);
	const [overflows, setOverflows] = React.useState(false);
	React.useLayoutEffect(() => {
		const el = ref.current;
		if (el) setOverflows(el.scrollHeight > el.clientHeight + 2);
	}, []);
	return (
		<div className="flex flex-col items-start gap-1.5">
			<div
				className={cn(!open && "overflow-hidden")}
				ref={ref}
				style={open ? undefined : { maxHeight: `${lines * 1.625}rem` }}
			>
				{children}
			</div>
			{(overflows || open) && (
				<button
					aria-expanded={open}
					className="cursor-pointer rounded-sm font-medium text-muted-foreground text-xs outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
					onClick={() => setOpen((v) => !v)}
					type="button"
				>
					{open ? "Show less" : "Read more"}
				</button>
			)}
		</div>
	);
}
