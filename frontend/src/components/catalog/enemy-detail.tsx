import { Link } from "@tanstack/react-router";
import { BoxesIcon, ShieldIcon } from "lucide-react";
import * as React from "react";
import { ArtGallery } from "@/components/catalog/art-gallery";
import { AttributeExplorer } from "@/components/catalog/attribute-explorer";
import {
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
import { RichText } from "@/components/rich-text/rich-text";
import { Button } from "@/components/ui/button";
import {
	Select,
	SelectItem,
	SelectPopup,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import type { EnemyDetail as Enemy, Resistances } from "@/lib/catalog";
import { studioLink } from "@/lib/studio";

const RESIST: { key: keyof Resistances; label: string; color: string }[] = [
	{ key: "physical", label: "Physical", color: "#9aa3ad" },
	{ key: "fire", label: "Heat", color: "#ff8e59" },
	{ key: "cryst", label: "Cryo", color: "#5ec8ff" },
	{ key: "pulse", label: "Electric", color: "#ffc000" },
	{ key: "natural", label: "Nature", color: "#8fd16a" },
];

/** Fixed attributes already shown as resistances. */
const RESIST_CODES = /(Resistance|DmgResistScalar)$/;

export default function EnemyDetail({
	data,
	tab,
	onTabChange,
	gender,
	onGenderChange,
}: DetailProps<Enemy>): React.ReactElement {
	const templateIds = Object.keys(data.attributeTemplates);
	const [templateId, setTemplateId] = React.useState(
		data.variants.find((v) => v.id === data.id)?.attrTemplateId ??
			templateIds[0],
	);
	const template = templateId ? data.attributeTemplates[templateId] : undefined;
	const hasModel = Boolean(data.model?.rigged || data.model?.parts.length);
	const abilities = data.abilities.filter(
		(a) => a.description.length > 0 || a.name,
	);

	return (
		<DetailPage>
			<DetailHero
				actions={
					<>
						<GenderToggle onChange={onGenderChange} value={gender} />
						{hasModel && (
							<Button
								render={<Link {...studioLink("enemies", data.id)} />}
								size="sm"
								variant="outline"
							>
								<BoxesIcon />
								Open in Studio
							</Button>
						)}
					</>
				}
				art={data.art.iconBig}
				artClassName="object-contain object-right p-4"
				chips={
					<>
						<InfoChip>{data.displayType.name}</InfoChip>
						{data.tags.map((t) => (
							<InfoChip key={t.code}>{t.name}</InfoChip>
						))}
					</>
				}
				icon={data.art.icon ?? data.art.adventureIcon}
				kind="enemies"
				name={data.name}
				subtitle={
					data.nickname && data.nickname !== data.name
						? data.nickname
						: undefined
				}
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
										<RichText
											as="p"
											className="text-pretty text-foreground/90 text-sm leading-relaxed"
											value={data.description}
										/>
									</Panel>
								)}
								{abilities.length > 0 && (
									<Section title="Abilities">
										<ul className="flex flex-col gap-2">
											{abilities.map((a) => (
												<li key={a.id}>
													<Panel className="flex flex-col gap-1 py-3">
														{a.name && (
															<span className="font-medium text-sm">
																{a.name}
															</span>
														)}
														<RichText
															className="text-foreground/85 text-sm"
															value={a.description}
														/>
													</Panel>
												</li>
											))}
										</ul>
									</Section>
								)}
								{template && (
									<Section title="Resistances">
										<ResistanceBars template={template} />
									</Section>
								)}
							</div>
						),
					},
					{
						value: "stats",
						label: "Stats",
						content: () =>
							template ? (
								<div className="flex flex-col gap-5">
									{templateIds.length > 1 && (
										<Select
											items={templateIds.map((t) => ({ value: t, label: t }))}
											onValueChange={(v) =>
												typeof v === "string" && setTemplateId(v)
											}
											value={templateId}
										>
											<SelectTrigger
												aria-label="Attribute template"
												className="w-auto min-w-56"
												size="sm"
											>
												<SelectValue />
											</SelectTrigger>
											<SelectPopup>
												{templateIds.map((t) => (
													<SelectItem key={t} value={t}>
														<span className="font-mono text-xs">{t}</span>
													</SelectItem>
												))}
											</SelectPopup>
										</Select>
									)}
									<ResistanceBars template={template} />
									<AttributeExplorer
										attributes={template.levels}
										fixed={Object.fromEntries(
											Object.entries(template.fixed).filter(
												([k]) => !RESIST_CODES.test(k),
											),
										)}
										fixedTitle="Fixed attributes"
									/>
								</div>
							) : (
								<p className="text-muted-foreground text-sm">
									No attribute data.
								</p>
							),
					},
					{
						value: "variants",
						label: "Variants",
						count: data.variants.length,
						content: () => (
							<div className="overflow-x-auto rounded-xl border bg-card">
								<table className="w-full text-sm">
									<thead className="border-b bg-muted/40 text-muted-foreground text-xs">
										<tr>
											<th
												className="px-3 py-2 text-start font-medium"
												scope="col"
											>
												Variant
											</th>
											<th
												className="px-3 py-2 text-start font-medium"
												scope="col"
											>
												Name
											</th>
											<th
												className="px-3 py-2 text-start font-medium"
												scope="col"
											>
												Attribute template
											</th>
										</tr>
									</thead>
									<tbody className="divide-y">
										{data.variants.map((v) => (
											<tr key={v.id}>
												<td className="px-3 py-2 font-mono text-xs">{v.id}</td>
												<td className="px-3 py-2">
													{v.name || (
														<span className="text-muted-foreground">—</span>
													)}
												</td>
												<td className="px-3 py-2">
													<button
														className="cursor-pointer font-mono text-xs underline-offset-2 hover:underline"
														onClick={() => {
															setTemplateId(v.attrTemplateId);
															onTabChange("stats");
														}}
														type="button"
													>
														{v.attrTemplateId}
													</button>
												</td>
											</tr>
										))}
									</tbody>
								</table>
							</div>
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
							<ModelPanel id={data.id} kind="enemies" model={data.model} />
						),
					},
				]}
			/>
		</DetailPage>
	);
}

function ResistanceBars({
	template,
}: {
	template: NonNullable<Enemy["attributeTemplates"][string]>;
}): React.ReactElement {
	return (
		<Panel className="flex flex-col gap-3">
			<div className="grid gap-3 sm:grid-cols-5">
				{RESIST.map((r) => {
					const value = template.resistances[r.key] ?? 0;
					return (
						<div className="flex flex-col gap-1.5" key={r.key}>
							<div className="flex items-baseline justify-between text-xs">
								<span className="text-muted-foreground">{r.label}</span>
								<span className="font-medium tabular-nums">{value}</span>
							</div>
							<div
								aria-hidden
								className="h-1.5 overflow-hidden rounded-full bg-muted"
							>
								<div
									className="h-full rounded-full"
									style={{
										width: `${Math.min(100, Math.max(0, value))}%`,
										background: r.color,
									}}
								/>
							</div>
						</div>
					);
				})}
			</div>
			{template.maxResilience ? (
				<div className="flex items-center gap-2 text-muted-foreground text-xs">
					<ShieldIcon className="size-3.5" />
					Max stagger resilience
					<span className="font-medium text-foreground tabular-nums">
						{template.maxResilience}
					</span>
				</div>
			) : null}
		</Panel>
	);
}
