import { Link } from "@tanstack/react-router";
import { BoxesIcon, LockIcon, MicIcon } from "lucide-react";
import * as React from "react";
import { VoiceLines } from "@/components/audio/voice-lines";
import { ArtGallery } from "@/components/catalog/art-gallery";
import { AttributeExplorer } from "@/components/catalog/attribute-explorer";
import { CostList } from "@/components/catalog/cost-list";
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
import { EntityImage } from "@/components/catalog/entity-image";
import { ModelPanel } from "@/components/catalog/model-panel";
import { ProfileText } from "@/components/catalog/profile-text";
import { SkillsPanel } from "@/components/catalog/skills-panel";
import { TagGlyph } from "@/components/catalog/tag-glyph";
import { RichText } from "@/components/rich-text/rich-text";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	attrLabel,
	type OperatorDetail as Operator,
	titleCaseShouting,
} from "@/lib/catalog";
import { formatNumber } from "@/lib/format";
import { studioLink } from "@/lib/studio";

const SKILL_ORDER = [
	"NormalAttack",
	"NormalSkill",
	"ComboSkill",
	"UltimateSkill",
];

const UNLOCK_LABELS: Record<string, (v: number) => string> = {
	ReachFavorability: (v) => `Trust ${formatNumber(v)}`,
	ReachLevel: (v) => `Level ${v}`,
	ReachBreakStage: (v) => `Promotion ${v}`,
};

function unlockLabel(unlock: { type: string; value: number }): string | null {
	if (unlock.type === "DefaultUnlock") return null;
	return (
		UNLOCK_LABELS[unlock.type]?.(unlock.value) ??
		`${unlock.type} ${unlock.value}`
	);
}

const ROMAN = ["", "I", "II", "III", "IV", "V", "VI"];

export default function OperatorDetail({
	data,
	tab,
	onTabChange,
	gender,
	onGenderChange,
	gendered,
}: DetailProps<Operator>): React.ReactElement {
	const stageLabel = (stage: number) =>
		stage === 0 ? "Base" : `P${ROMAN[stage] ?? stage}`;
	const skills = React.useMemo(
		() =>
			[...data.skills].sort(
				(a, b) =>
					SKILL_ORDER.indexOf(a.type ?? "") - SKILL_ORDER.indexOf(b.type ?? ""),
			),
		[data.skills],
	);
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
								render={<Link {...studioLink("operators", data.id)} />}
								size="sm"
								variant="outline"
							>
								<BoxesIcon />
								Open in Studio
							</Button>
						)}
					</>
				}
				art={data.art.portrait ?? data.art.infoBackground}
				chips={
					<>
						<InfoChip icon={data.profession.icon}>
							{data.profession.name}
						</InfoChip>
						<InfoChip color={data.element.color} icon={data.element.icon}>
							{data.element.name}
						</InfoChip>
						<InfoChip>
							<TagGlyph
								className="-ms-1 size-4"
								tag={{ kind: "weaponType", ...data.weaponType }}
							/>
							{data.weaponType.name}
						</InfoChip>
						{data.department && (
							<InfoChip>{titleCaseShouting(data.department)}</InfoChip>
						)}
					</>
				}
				icon={data.art.icon ?? data.art.roundIcon}
				kind="operators"
				name={data.name}
				rarity={data.rarity}
				subtitle={
					data.engName && data.engName !== data.name ? data.engName : undefined
				}
			/>
			<DetailTabs
				onTabChange={onTabChange}
				tab={tab}
				tabs={[
					{
						value: "overview",
						label: "Overview",
						content: () => <Overview data={data} />,
					},
					{
						value: "stats",
						label: "Stats",
						content: () => (
							<div className="flex flex-col gap-6">
								<AttributeExplorer
									attributes={data.attributes}
									stageLabel={stageLabel}
								/>
								<Promotions data={data} />
							</div>
						),
					},
					{
						value: "skills",
						label: "Skills",
						count: skills.length,
						content: () => <SkillsPanel skills={skills} />,
					},
					{
						value: "talents",
						label: "Talents & Potentials",
						content: () => <Talents data={data} />,
					},
					{
						value: "voice",
						label: "Voice lines",
						count: data.voiceLines.length,
						content: () => <VoiceLines data={data} />,
					},
					{
						value: "art",
						label: "Art",
						count: Object.values(data.art).filter(Boolean).length,
						content: () => (
							<ArtGallery
								art={data.art as Record<string, string>}
								name={data.name}
							/>
						),
					},
					{
						value: "model",
						label: "Model",
						content: () => (
							<ModelPanel
								id={data.id}
								kind="operators"
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

function Overview({ data }: { data: Operator }): React.ReactElement {
	const va = data.voiceActors ?? {};
	return (
		<div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
			<div className="flex flex-col gap-4">
				{data.profile.map((entry) => {
					const lock = unlockLabel(entry.unlock);
					return (
						<Panel className="flex flex-col gap-2" key={entry.id}>
							<div className="flex items-center justify-between gap-2">
								<h3 className="font-heading font-semibold text-muted-foreground text-xs uppercase tracking-wider">
									{entry.title}
								</h3>
								{lock && (
									<Badge variant="outline">
										<LockIcon />
										{lock}
									</Badge>
								)}
							</div>
							<ProfileText value={entry.text} />
						</Panel>
					);
				})}
			</div>
			<aside className="flex flex-col gap-4 lg:sticky lg:top-4 lg:self-start">
				<Panel>
					<Facts
						items={[
							{ label: "Class", value: data.profession.name },
							{ label: "Element", value: data.element.name },
							{ label: "Weapon", value: data.weaponType.name },
							{
								label: "Faction",
								value: data.department && titleCaseShouting(data.department),
							},
							{
								label: "Main attribute",
								value: data.mainAttribute
									? attrLabel(data.mainAttribute)
									: null,
							},
							{
								label: "Sub attribute",
								value: data.subAttribute ? attrLabel(data.subAttribute) : null,
							},
							{
								label: "Default weapon",
								value: data.defaultWeapon ? (
									<Link
										className="font-medium underline-offset-2 hover:underline"
										params={{ kind: "weapons", id: data.defaultWeapon.id }}
										to="/catalog/$kind/$id"
									>
										{data.defaultWeapon.name}
									</Link>
								) : null,
							},
							{
								label: "Tags",
								value: data.battleTags.length ? (
									<span className="flex flex-wrap gap-1">
										{data.battleTags.map((t) => (
											<Badge key={t.code} variant="secondary">
												{t.name}
											</Badge>
										))}
									</span>
								) : null,
							},
						]}
					/>
				</Panel>
				{(va.cn || va.en || va.jp || va.kr) && (
					<Panel className="flex flex-col gap-3">
						<h3 className="flex items-center gap-2 font-medium text-sm">
							<MicIcon className="size-4 text-muted-foreground" />
							Voice actors
						</h3>
						<Facts
							items={[
								{ label: "English", value: va.en },
								{ label: "Japanese", value: va.jp },
								{ label: "Chinese", value: va.cn },
								{ label: "Korean", value: va.kr },
							]}
						/>
					</Panel>
				)}
			</aside>
		</div>
	);
}

function Promotions({ data }: { data: Operator }): React.ReactElement | null {
	if (data.promotions.length === 0) return null;
	return (
		<Section title="Promotions & outfitting">
			<ol className="grid gap-3 md:grid-cols-2">
				{data.promotions.map((p) => (
					<li key={p.id}>
						<Panel className="flex h-full flex-col gap-2">
							<div className="flex items-center justify-between gap-2">
								<span className="font-medium text-sm">{p.name}</span>
								<Badge variant={p.type === "CharBreak" ? "info" : "secondary"}>
									{p.type === "CharBreak" ? "Promotion" : "Outfitting"}
								</Badge>
							</div>
							<RichText
								className="text-muted-foreground text-sm"
								value={p.description}
							/>
							<CostList costs={p.cost} />
						</Panel>
					</li>
				))}
			</ol>
		</Section>
	);
}

function Talents({ data }: { data: Operator }): React.ReactElement {
	const groups = React.useMemo(() => {
		const map = new Map<number, Operator["talents"]>();
		for (const t of data.talents) {
			const list = map.get(t.index) ?? [];
			list.push(t);
			map.set(t.index, list);
		}
		return [...map.values()].map((list) =>
			[...list].sort((a, b) => a.level - b.level),
		);
	}, [data.talents]);

	return (
		<div className="flex flex-col gap-8">
			{groups.length > 0 && (
				<Section title="Talents">
					<div className="grid gap-3 md:grid-cols-2">
						{groups.map((levels) => {
							const first = levels[0];
							if (!first) return null;
							return (
								<Panel className="flex flex-col gap-3" key={first.id}>
									<div className="flex items-center gap-3">
										{first.icon && (
											<EntityImage
												className="size-10 shrink-0 rounded-lg bg-neutral-900 p-1 dark:bg-neutral-800"
												path={first.icon}
												size={128}
											/>
										)}
										<h3 className="font-heading font-semibold">{first.name}</h3>
									</div>
									<ol className="flex flex-col gap-3">
										{levels.map((t) => (
											<li
												className="flex flex-col gap-1.5 border-s-2 ps-3"
												key={t.id}
											>
												<div className="flex items-center gap-2 text-xs">
													<span className="font-medium">Rank {t.level}</span>
													<span className="text-muted-foreground">
														· Promotion {ROMAN[t.breakStage] ?? t.breakStage}
													</span>
												</div>
												<RichText
													as="p"
													className="text-foreground/90 text-sm leading-relaxed"
													value={t.description}
												/>
												<CostList costs={t.cost} />
											</li>
										))}
									</ol>
								</Panel>
							);
						})}
					</div>
				</Section>
			)}
			{data.potentials.length > 0 && (
				<Section title="Potentials">
					<ol className="flex flex-col gap-2">
						{data.potentials.map((p) => (
							<li key={p.effectId}>
								<Panel className="flex gap-4 py-3">
									<span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-muted font-heading font-semibold text-sm tabular-nums">
										{p.level}
									</span>
									<div className="flex min-w-0 flex-col gap-0.5">
										<span className="font-medium text-sm">{p.name}</span>
										<RichText
											className="text-foreground/85 text-sm leading-relaxed"
											value={p.description}
										/>
									</div>
								</Panel>
							</li>
						))}
					</ol>
				</Section>
			)}
			{data.attributeNodes.length > 0 && (
				<Section title="Attribute nodes">
					<div className="grid gap-3 md:grid-cols-2">
						{data.attributeNodes.map((node) => (
							<Panel className="flex flex-col gap-2" key={node.id}>
								<div className="flex items-center justify-between gap-2">
									<span className="font-medium text-sm">{node.title}</span>
									<Badge variant="outline">
										Promotion {ROMAN[node.breakStage] ?? node.breakStage}
									</Badge>
								</div>
								<RichText
									className="text-foreground/85 text-sm"
									value={node.description}
								/>
								<CostList costs={node.cost} />
							</Panel>
						))}
					</div>
				</Section>
			)}
		</div>
	);
}
