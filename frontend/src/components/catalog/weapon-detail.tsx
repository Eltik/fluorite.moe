import { Link } from "@tanstack/react-router";
import { BoxesIcon } from "lucide-react";
import * as React from "react";
import { ArtGallery } from "@/components/catalog/art-gallery";
import { AttributeExplorer } from "@/components/catalog/attribute-explorer";
import { CostList } from "@/components/catalog/cost-list";
import {
	ClampedText,
	DetailHero,
	DetailPage,
	type DetailProps,
	DetailTabs,
	GenderToggle,
	InfoChip,
	Panel,
	Section,
} from "@/components/catalog/detail-layout";
import { ModelPanel } from "@/components/catalog/model-panel";
import { SkillsPanel } from "@/components/catalog/skills-panel";
import { RichText } from "@/components/rich-text/rich-text";
import { Button } from "@/components/ui/button";
import type { AttrSeries, WeaponDetail as Weapon } from "@/lib/catalog";
import { studioLink } from "@/lib/studio";

export default function WeaponDetail({
	data,
	tab,
	onTabChange,
	gender,
	onGenderChange,
}: DetailProps<Weapon>): React.ReactElement {
	const atk = React.useMemo<AttrSeries>(
		() => ({
			level: data.atk.level,
			breakStage: [],
			series: { Atk: data.atk.baseAtk },
			constants: {},
		}),
		[data.atk],
	);
	const hasModel = Boolean(data.model?.rigged || data.model?.parts.length);
	const breakthroughs = data.breakthroughs.filter(
		(b) => b.gold > 0 || b.cost.some((c) => c.count > 0),
	);

	return (
		<DetailPage>
			<DetailHero
				actions={
					<>
						<GenderToggle onChange={onGenderChange} value={gender} />
						{hasModel && (
							<Button
								render={<Link {...studioLink("weapons", data.id)} />}
								size="sm"
								variant="outline"
							>
								<BoxesIcon />
								Open in Studio
							</Button>
						)}
					</>
				}
				art={data.art.gacha ?? data.art.iconBig}
				artClassName="object-contain object-right p-4"
				chips={
					<>
						<InfoChip>{data.weaponType.name}</InfoChip>
						<InfoChip>Max Lv {data.maxLevel}</InfoChip>
					</>
				}
				icon={data.art.iconBig ?? data.art.icon}
				kind="weapons"
				name={data.name}
				rarity={data.rarity}
			/>
			<DetailTabs
				onTabChange={onTabChange}
				tab={tab}
				tabs={[
					{
						value: "overview",
						label: "Overview",
						content: () => (
							<div className="flex flex-col gap-6">
								{data.description.length > 0 && (
									<Panel>
										<ClampedText>
											<RichText
												as="p"
												className="text-pretty text-foreground/90 text-sm italic leading-relaxed"
												value={data.description}
											/>
										</ClampedText>
									</Panel>
								)}
								{data.operators && data.operators.length > 0 && (
									<Section title="Related operators">
										<div className="flex flex-wrap gap-2">
											{data.operators.map((op) => (
												<Button
													key={op.id}
													render={
														<Link
															params={{ kind: "operators", id: op.id }}
															to="/catalog/$kind/$id"
														/>
													}
													size="sm"
													variant="outline"
												>
													{op.name}
												</Button>
											))}
										</div>
									</Section>
								)}
								<Section title="Base ATK">
									<AttributeExplorer
										attributes={atk}
										markerLabel="breakthroughs"
										markerLevels={data.breakthroughs.map((b) => b.level)}
									/>
								</Section>
								{breakthroughs.length > 0 && (
									<Section title="Breakthroughs">
										<ol className="grid gap-3 md:grid-cols-2">
											{breakthroughs.map((b) => (
												<li key={b.stage}>
													<Panel className="flex h-full flex-col gap-2">
														<div className="flex items-baseline justify-between">
															<span className="font-medium text-sm">
																Breakthrough {b.stage}
															</span>
															<span className="text-muted-foreground text-xs">
																at Lv {b.level}
															</span>
														</div>
														<CostList costs={b.cost} gold={b.gold} />
													</Panel>
												</li>
											))}
										</ol>
									</Section>
								)}
							</div>
						),
					},
					{
						value: "skills",
						label: "Skills",
						count: data.skills.length,
						content: () => (
							<SkillsPanel
								skills={data.skills}
								typeLabel={(_, i) =>
									i === data.skills.length - 1
										? "Weapon skill"
										: `Attribute ${i + 1}`
								}
							/>
						),
					},
					{
						value: "art",
						label: "Art",
						content: () => <ArtGallery art={data.art} name={data.name} />,
					},
					{
						value: "model",
						label: "Model",
						content: () => (
							<ModelPanel id={data.id} kind="weapons" model={data.model} />
						),
					},
				]}
			/>
		</DetailPage>
	);
}
