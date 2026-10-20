import { Link } from "@tanstack/react-router";
import {
	ArrowRightIcon,
	BoxesIcon,
	MousePointer2Icon,
	ShuffleIcon,
} from "lucide-react";
import type * as React from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { thumbUrl } from "@/lib/api";
import { type ListItem, RARITY_COLORS } from "@/lib/catalog";
import { studioLink } from "@/lib/studio";
import { cn } from "@/lib/utils";

/** Full-body splash art; every operator ships one under this folder. */
export function portraitPath(id: string): string {
	return `beyond/dynamicassets/gameplay/ui/sprites/characterportrait/${id}.png`;
}

const TAG_ORDER = ["profession", "element", "weaponType"];

/**
 * Studio-first landing hero: a featured operator's splash art with a direct
 * way into the 3D Studio. Server-rendered from the operator list, so it
 * paints with the first HTML response.
 */
export function FeaturedHero({
	item,
	onShuffle,
}: {
	item: ListItem | undefined;
	onShuffle?: () => void;
}): React.ReactElement {
	const accent = (item?.rarity && RARITY_COLORS[item.rarity]) || "#8b5cf6";
	const src = item ? thumbUrl(portraitPath(item.id), 512) : undefined;
	const tags = item
		? [...item.tags]
				.filter((t) => TAG_ORDER.includes(t.kind))
				.sort((a, b) => TAG_ORDER.indexOf(a.kind) - TAG_ORDER.indexOf(b.kind))
		: [];

	return (
		<section
			aria-labelledby="hero-heading"
			className="relative isolate overflow-hidden rounded-3xl border bg-card"
			style={{ "--hero-accent": accent } as React.CSSProperties}
		>
			{/* Decorative backdrop: rarity glow + faint stage grid. */}
			<div
				aria-hidden="true"
				className="-z-10 pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_60%_80%_at_75%_45%,color-mix(in_oklab,var(--hero-accent)_22%,transparent),transparent_70%)] transition-[background] duration-500"
			/>
			<div
				aria-hidden="true"
				className="-z-10 pointer-events-none absolute inset-0 bg-[linear-gradient(to_right,var(--border)_1px,transparent_1px),linear-gradient(to_bottom,var(--border)_1px,transparent_1px)] bg-size-[48px_48px] [mask-image:radial-gradient(ellipse_70%_90%_at_70%_60%,black,transparent_75%)]"
			/>

			{src && (
				<img
					alt={item ? `${item.name} splash art` : ""}
					className={cn(
						"-z-10 pointer-events-none absolute right-[-8%] bottom-0 h-[115%] max-w-none select-none object-contain object-bottom opacity-60 [mask-image:linear-gradient(to_right,transparent,black_28%)] sm:right-[-2%] md:right-[2%] md:h-[112%] md:opacity-100",
					)}
					decoding="async"
					draggable={false}
					fetchPriority="high"
					height={512}
					key={src}
					src={src}
					width={512}
				/>
			)}

			{/* Small screens stack text over the art: a scrim keeps it legible. */}
			<div
				aria-hidden="true"
				className="-z-10 pointer-events-none absolute inset-0 bg-[linear-gradient(to_top,var(--card)_40%,color-mix(in_oklab,var(--card)_55%,transparent))] md:hidden"
			/>

			<div className="flex min-h-[22rem] flex-col justify-end gap-5 p-6 sm:p-8 md:min-h-[26rem] md:max-w-[55%] md:justify-center md:p-10 lg:max-w-[50%]">
				<div className="flex flex-col gap-3">
					<p className="flex items-center gap-2 font-medium text-brand-foreground text-xs uppercase tracking-[0.14em]">
						<BoxesIcon aria-hidden="true" className="size-3.5" />
						fluorite Studio
					</p>
					{item ? (
						<>
							<h2
								className="text-balance font-semibold text-4xl leading-[1.05] tracking-tight sm:text-5xl"
								id="hero-heading"
							>
								{item.name}
							</h2>
							<div className="flex flex-wrap items-center gap-2">
								{item.rarity ? (
									<span
										className="me-1 text-sm tracking-[0.15em]"
										style={{ color: accent }}
									>
										<span aria-hidden="true">{"★".repeat(item.rarity)}</span>
										<span className="sr-only">{item.rarity}-star operator</span>
									</span>
								) : null}
								{tags.map((tag) => (
									<Badge
										className="bg-background/60 backdrop-blur-sm"
										key={`${tag.kind}:${tag.code}`}
										variant="outline"
									>
										{tag.name}
									</Badge>
								))}
							</div>
						</>
					) : (
						<>
							<h2 className="sr-only" id="hero-heading">
								Featured operator
							</h2>
							<Skeleton className="h-12 w-64" />
							<Skeleton className="h-5 w-48" />
						</>
					)}
					<p className="max-w-md text-pretty text-muted-foreground leading-relaxed">
						Orbit, light and inspect every operator, enemy and weapon as the
						game's own 3D model, then dive into their stats, voice lines and
						story.
					</p>
				</div>

				<div className="flex flex-wrap items-center gap-2">
					{item ? (
						<>
							<Button
								render={<Link {...studioLink("operators", item.id)} />}
								size="lg"
							>
								<MousePointer2Icon />
								Open {item.name} in Studio
							</Button>
							<Button
								render={
									<Link
										params={{ kind: "operators", id: item.id }}
										to="/catalog/$kind/$id"
									/>
								}
								size="lg"
								variant="outline"
							>
								Profile
								<ArrowRightIcon />
							</Button>
							{onShuffle && (
								<Button
									aria-label="Feature another operator"
									onClick={onShuffle}
									size="icon-lg"
									title="Feature another operator"
									variant="ghost"
								>
									<ShuffleIcon />
								</Button>
							)}
						</>
					) : (
						<Button render={<Link to="/studio" />} size="lg">
							<BoxesIcon />
							Open the Studio
						</Button>
					)}
				</div>
			</div>
		</section>
	);
}
