import { Link } from "@tanstack/react-router";
import {
	ArrowRightIcon,
	BoxesIcon,
	MapPinIcon,
	ZapIcon,
	ZapOffIcon,
} from "lucide-react";
import * as React from "react";
import { ArtGallery } from "@/components/catalog/art-gallery";
import {
	DetailHero,
	DetailPage,
	type DetailProps,
	DetailTabs,
	Facts,
	GenderToggle,
	InfoChip,
	Panel,
	Section,
} from "@/components/catalog/detail-layout";
import { ModelPanel } from "@/components/catalog/model-panel";
import { RichText } from "@/components/rich-text/rich-text";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	type FacilityDetail as Facility,
	facilityTint,
	humanizeCode,
	type ItemDetail as Item,
	KIND_SINGULAR,
	type NpcDetail as Npc,
} from "@/lib/catalog";
import { studioLink } from "@/lib/studio";

const SYSTEM_LABELS: Record<string, string> = {
	Dungeon: "Protocol Space",
	Gacha: "Acquisition Center",
	DailyMission: "Missions",
};

// ---------------------------------------------------------------- items

export function ItemDetail({
	data,
	gender,
	onGenderChange,
	gendered,
}: DetailProps<Item>): React.ReactElement {
	const groups = React.useMemo(() => {
		const map = new Map<string, Item["obtainWays"]>();
		for (const way of data.obtainWays) {
			const key = way.system ?? "Other";
			const list = map.get(key) ?? [];
			// The API lists first-clear and repeat sources separately with the
			// same text; show each description once.
			if (!list.some((w) => w.description === way.description)) list.push(way);
			map.set(key, list);
		}
		return [...map];
	}, [data.obtainWays]);

	return (
		<DetailPage>
			<DetailHero
				actions={
					gendered && (
						<GenderToggle onChange={onGenderChange} showLabel value={gender} />
					)
				}
				chips={<InfoChip>{data.itemType.name}</InfoChip>}
				icon={data.art.iconBig ?? data.art.icon}
				kind="items"
				name={data.name}
				rarity={data.rarity}
			/>
			<div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
				<div className="flex flex-col gap-4">
					{data.description.length > 0 && (
						<Panel>
							<RichText
								as="p"
								className="text-pretty text-sm leading-relaxed"
								value={data.description}
							/>
						</Panel>
					)}
					{data.flavor.length > 0 && (
						<Panel className="bg-muted/40">
							<RichText
								as="p"
								className="text-pretty text-muted-foreground text-sm italic leading-relaxed"
								value={data.flavor}
							/>
						</Panel>
					)}
					<Section title="How to obtain">
						{groups.length === 0 ? (
							<p className="text-muted-foreground text-sm">
								{data.noObtainHint || "No acquisition source is listed."}
							</p>
						) : (
							<ul className="flex flex-col gap-2">
								{groups.map(([system, ways]) => (
									<li key={system}>
										<Panel className="flex flex-col gap-2 py-3">
											<span className="text-muted-foreground text-xs uppercase tracking-wide">
												{SYSTEM_LABELS[system] ?? system}
											</span>
											<ul className="flex flex-col gap-1">
												{ways.map((w) => (
													<li
														className="flex items-center gap-2 text-sm"
														key={w.id}
													>
														<MapPinIcon className="size-3.5 shrink-0 text-muted-foreground" />
														{w.description}
													</li>
												))}
											</ul>
										</Panel>
									</li>
								))}
							</ul>
						)}
					</Section>
				</div>
				<aside className="flex flex-col gap-4">
					<Panel>
						<Facts
							items={[
								{ label: "Type", value: data.itemType.name },
								{
									label: "Rarity",
									value: data.rarity ? `${data.rarity}★` : null,
								},
								{
									label: "ID",
									value: <span className="font-mono text-xs">{data.id}</span>,
								},
							]}
						/>
					</Panel>
					{data.related && (
						<Button
							className="justify-between"
							render={
								<Link
									params={{ kind: data.related.kind, id: data.related.id }}
									to="/catalog/$kind/$id"
								/>
							}
							variant="outline"
						>
							View {KIND_SINGULAR[data.related.kind]?.toLowerCase() ?? "entry"}
							<ArrowRightIcon />
						</Button>
					)}
					{Object.values(data.art).filter(Boolean).length > 1 && (
						<Section title="Art">
							<ArtGallery art={data.art} name={data.name} />
						</Section>
					)}
				</aside>
			</div>
		</DetailPage>
	);
}

// ---------------------------------------------------------------- npcs

export function NpcDetail({
	data,
	tab,
	onTabChange,
	gender,
	onGenderChange,
	gendered,
}: DetailProps<Npc>): React.ReactElement {
	const hasModel = Boolean(data.model?.rigged || data.model?.parts.length);
	return (
		<DetailPage>
			<DetailHero
				actions={
					<>
						{gendered && (
							<GenderToggle
								onChange={onGenderChange}
								showLabel
								value={gender}
							/>
						)}
						{hasModel && (
							<Button
								render={<Link {...studioLink("npcs", data.id)} />}
								size="sm"
								variant="outline"
							>
								<BoxesIcon />
								Open in Studio
							</Button>
						)}
					</>
				}
				chips={
					<>
						{data.title && <InfoChip>{data.title}</InfoChip>}
						{data.faction && <InfoChip>{data.faction}</InfoChip>}
					</>
				}
				icon={data.icon}
				kind="npcs"
				name={data.name}
			/>
			<DetailTabs
				onTabChange={onTabChange}
				tab={tab}
				tabs={[
					{
						value: "overview",
						label: "Overview",
						content: () => (
							<Panel className="max-w-xl">
								<Facts
									items={[
										{ label: "Title", value: data.title },
										{ label: "Faction", value: data.faction },
										{
											label: "Data key",
											value: data.dataKey && (
												<span className="font-mono text-xs">
													{data.dataKey}
												</span>
											),
										},
										{
											label: "Group",
											value: data.groupId && (
												<span className="font-mono text-xs">
													{data.groupId}
												</span>
											),
										},
										{
											label: "ID",
											value: (
												<span className="font-mono text-xs">{data.id}</span>
											),
										},
									]}
								/>
							</Panel>
						),
					},
					{
						value: "model",
						label: "Model",
						content: () => (
							<ModelPanel
								id={data.id}
								kind="npcs"
								model={data.model}
								name={data.name}
							/>
						),
					},
				]}
			/>
		</DetailPage>
	);
}

// ---------------------------------------------------------------- facilities

export function FacilityDetail({
	data,
	gender,
	onGenderChange,
	gendered,
}: DetailProps<Facility>): React.ReactElement {
	const { range } = data;
	const name = data.name === data.id ? humanizeCode(data.id) : data.name;
	const type = humanizeCode(data.type);
	return (
		<DetailPage>
			<DetailHero
				actions={
					gendered && (
						<GenderToggle onChange={onGenderChange} showLabel value={gender} />
					)
				}
				art={data.art.image}
				artClassName="object-contain object-right p-4"
				chips={
					<>
						<InfoChip color={facilityTint(data.type)}>{type}</InfoChip>
						<InfoChip>
							{data.needPower ? (
								<ZapIcon className="size-3.5" />
							) : (
								<ZapOffIcon className="size-3.5" />
							)}
							{data.needPower
								? `${data.powerConsume} power`
								: "No power needed"}
						</InfoChip>
					</>
				}
				icon={data.art.iconBig ?? data.art.icon}
				kind="facilities"
				name={name}
			/>
			<div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
				<div className="flex flex-col gap-4">
					{data.description.length > 0 && (
						<Panel>
							<RichText
								as="p"
								className="text-pretty text-sm leading-relaxed"
								value={data.description}
							/>
						</Panel>
					)}
					<Section title="Footprint">
						<div className="grid grid-cols-3 gap-2">
							{(
								[
									["Width", range.width],
									["Depth", range.depth],
									["Height", range.height],
								] as const
							).map(([label, value]) => (
								<Panel
									className="flex flex-col items-center gap-0.5 py-3"
									key={label}
								>
									<span className="font-heading font-semibold text-xl tabular-nums">
										{value}
									</span>
									<span className="text-muted-foreground text-xs">{label}</span>
								</Panel>
							))}
						</div>
					</Section>
					{Object.values(data.art).filter(Boolean).length > 0 && (
						<Section title="Art">
							<ArtGallery art={data.art} name={name} />
						</Section>
					)}
				</div>
				<aside>
					<Panel>
						<Facts
							items={[
								{ label: "Type", value: type },
								{
									label: "Power",
									value: data.needPower ? (
										<span className="tabular-nums">{data.powerConsume}</span>
									) : (
										<Badge variant="secondary">Not required</Badge>
									),
								},
								{ label: "Bandwidth", value: data.bandwidth },
								{ label: "Input ports", value: data.inputPorts },
								{ label: "Output ports", value: data.outputPorts },
								{
									label: "ID",
									value: <span className="font-mono text-xs">{data.id}</span>,
								},
							]}
						/>
					</Panel>
				</aside>
			</div>
		</DetailPage>
	);
}
