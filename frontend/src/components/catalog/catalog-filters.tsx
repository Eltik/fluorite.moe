import { CheckIcon, ChevronDownIcon, StarIcon, XIcon } from "lucide-react";
import * as React from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import { rarityColor, TAG_KIND_LABELS } from "@/lib/catalog";
import { cn } from "@/lib/utils";

export interface FacetValue {
	code: string;
	name: string;
	/** Matches with the other active filters applied. */
	count: number;
	/** Entries carrying this value overall. */
	total?: number;
}

export interface Facet {
	kind: string;
	values: FacetValue[];
}

/** Facets with more values than this collapse into a searchable popover. */
const INLINE_LIMIT = 12;
/** How many leading values of a large facet stay visible as chips. */
const FEATURED = 8;

export function FilterChip({
	pressed,
	onPressedChange,
	children,
	count,
	className,
	style,
}: {
	pressed: boolean;
	onPressedChange: (pressed: boolean) => void;
	children: React.ReactNode;
	count?: number;
	className?: string;
	style?: React.CSSProperties;
}): React.ReactElement {
	return (
		<button
			aria-pressed={pressed}
			className={cn(
				"inline-flex h-7 shrink-0 cursor-pointer select-none items-center gap-1.5 rounded-full border px-2.5 font-medium text-xs outline-none transition-[background-color,border-color,color] focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:h-8",
				pressed
					? "border-foreground/20 bg-foreground text-background hover:bg-foreground/90"
					: "border-input bg-background text-foreground/80 hover:bg-accent hover:text-foreground dark:bg-input/32",
				!pressed && count === 0 && "opacity-50",
				className,
			)}
			onClick={() => onPressedChange(!pressed)}
			style={style}
			type="button"
		>
			{children}
			{count !== undefined && (
				<span
					className={cn(
						"tabular-nums",
						pressed ? "text-background/64" : "text-muted-foreground",
					)}
				>
					{count}
				</span>
			)}
		</button>
	);
}

function FacetPopover({
	facet,
	selected,
	onToggle,
	triggerLabel,
}: {
	facet: Facet;
	selected: Set<string>;
	onToggle: (key: string) => void;
	triggerLabel?: string;
}): React.ReactElement {
	const [filter, setFilter] = React.useState("");
	const label = TAG_KIND_LABELS[facet.kind] ?? facet.kind;
	const chosen = facet.values.filter((v) =>
		selected.has(`${facet.kind}:${v.code}`),
	);
	const needle = filter.trim().toLowerCase();
	const visible = needle
		? facet.values.filter(
				(v) =>
					v.name.toLowerCase().includes(needle) ||
					v.code.toLowerCase().includes(needle),
			)
		: facet.values;

	return (
		<Popover>
			<PopoverTrigger
				render={
					<Button
						className={cn(
							"h-7 rounded-full px-2.5 text-xs sm:h-7",
							chosen.length > 0 && "border-foreground/24",
						)}
						size="sm"
						variant="outline"
					/>
				}
			>
				{triggerLabel ?? label}
				{chosen.length > 0 ? (
					<span className="max-w-40 truncate text-muted-foreground">
						{chosen.length === 1
							? chosen[0]?.name
							: `${chosen.length} selected`}
					</span>
				) : (
					<span className="text-muted-foreground tabular-nums">
						{facet.values.length}
					</span>
				)}
				<ChevronDownIcon className="size-3.5 opacity-60" />
			</PopoverTrigger>
			<PopoverPopup align="start" className="w-72 p-0">
				<div className="-mx-4 -my-4 flex flex-col">
					<div className="border-b p-2">
						<Input
							aria-label={`Filter ${label.toLowerCase()} options`}
							autoFocus
							onChange={(event) => setFilter(event.target.value)}
							placeholder={`Find a ${label.toLowerCase()}…`}
							size="sm"
							value={filter}
						/>
					</div>
					<ul aria-label={label} className="max-h-72 overflow-y-auto p-1">
						{visible.map((value) => {
							const key = `${facet.kind}:${value.code}`;
							const on = selected.has(key);
							return (
								<li key={key}>
									<button
										aria-pressed={on}
										className="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-start text-sm outline-none hover:bg-accent focus-visible:bg-accent"
										onClick={() => onToggle(key)}
										type="button"
									>
										<span
											className={cn(
												"flex size-4 shrink-0 items-center justify-center rounded-[4px] border",
												on
													? "border-foreground bg-foreground text-background"
													: "border-input",
											)}
										>
											{on && <CheckIcon className="size-3" />}
										</span>
										<span className="min-w-0 flex-1 truncate">
											{value.name}
										</span>
										<span
											className={cn(
												"text-muted-foreground text-xs tabular-nums",
												value.count === 0 && "opacity-50",
											)}
										>
											{value.count}
										</span>
									</button>
								</li>
							);
						})}
						{visible.length === 0 && (
							<li className="px-2 py-6 text-center text-muted-foreground text-sm">
								No matches
							</li>
						)}
					</ul>
				</div>
			</PopoverPopup>
		</Popover>
	);
}

export function CatalogFilters({
	facets,
	selected,
	onToggleTag,
	rarities,
	selectedRarities,
	onToggleRarity,
	rarityCounts,
	onClear,
}: {
	facets: Facet[];
	selected: Set<string>;
	onToggleTag: (key: string) => void;
	rarities: number[];
	selectedRarities: Set<number>;
	onToggleRarity: (rarity: number) => void;
	/** Matches per rarity with the other filters applied. */
	rarityCounts?: Map<number, number>;
	onClear?: () => void;
}): React.ReactElement | null {
	const inline = facets.filter((f) => f.values.length <= INLINE_LIMIT);
	// Large facets keep their leading values as a visible row of chips, with
	// the rest behind a searchable popover.
	const large = facets.filter((f) => f.values.length > INLINE_LIMIT);
	if (facets.length === 0 && rarities.length === 0) return null;

	return (
		<div className="flex flex-col gap-2.5">
			{inline.map((facet) => (
				<FilterRow
					key={facet.kind}
					label={TAG_KIND_LABELS[facet.kind] ?? facet.kind}
				>
					{facet.values.map((value) => {
						const key = `${facet.kind}:${value.code}`;
						return (
							<FilterChip
								count={value.count}
								key={key}
								onPressedChange={() => onToggleTag(key)}
								pressed={selected.has(key)}
							>
								{value.name}
							</FilterChip>
						);
					})}
				</FilterRow>
			))}
			{large.map((facet) => {
				const featured = facet.values.slice(0, FEATURED);
				const featuredCodes = new Set(featured.map((v) => v.code));
				const rest = {
					...facet,
					values: facet.values.filter((v) => !featuredCodes.has(v.code)),
				};
				return (
					<FilterRow
						key={facet.kind}
						label={TAG_KIND_LABELS[facet.kind] ?? facet.kind}
					>
						{featured.map((value) => {
							const key = `${facet.kind}:${value.code}`;
							return (
								<FilterChip
									count={value.count}
									key={key}
									onPressedChange={() => onToggleTag(key)}
									pressed={selected.has(key)}
								>
									{value.name}
								</FilterChip>
							);
						})}
						<FacetPopover
							facet={rest}
							onToggle={onToggleTag}
							selected={selected}
							triggerLabel="More"
						/>
					</FilterRow>
				);
			})}
			{rarities.length > 1 && (
				<FilterRow label="Rarity">
					{rarities.length > 1 &&
						rarities.map((rarity) => (
							<FilterChip
								count={rarityCounts?.get(rarity) ?? 0}
								key={rarity}
								onPressedChange={() => onToggleRarity(rarity)}
								pressed={selectedRarities.has(rarity)}
								style={
									{ "--rarity": rarityColor(rarity) } as React.CSSProperties
								}
							>
								<span className="tabular-nums">{rarity}</span>
								<StarIcon
									aria-label="stars"
									className="size-3 fill-(--rarity) stroke-none"
								/>
							</FilterChip>
						))}
				</FilterRow>
			)}
			{onClear && (
				<div>
					<Button
						className="h-7 px-2 text-xs"
						onClick={onClear}
						size="xs"
						variant="ghost"
					>
						<XIcon />
						Clear filters
					</Button>
				</div>
			)}
		</div>
	);
}

function FilterRow({
	label,
	children,
}: {
	label: string;
	children: React.ReactNode;
}): React.ReactElement {
	return (
		<fieldset className="flex min-w-0 items-start gap-3">
			<legend className="sr-only">{label}</legend>
			<span
				aria-hidden
				className="w-14 shrink-0 pt-1.5 text-muted-foreground text-xs"
			>
				{label}
			</span>
			<div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
				{children}
			</div>
		</fieldset>
	);
}
