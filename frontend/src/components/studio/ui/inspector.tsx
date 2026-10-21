/**
 * The mesh detail inspector: geometry, material and texture slots of the
 * picked mesh, with a channel viewer for any texture. Built only while open,
 * from the textures the model already holds.
 */
import {
	CrosshairIcon,
	EyeOffIcon,
	ImageIcon,
	ScanSearchIcon,
	SearchIcon,
	XIcon,
} from "lucide-react";
import * as React from "react";
import type { Texture } from "three";
import {
	type ColorDetail,
	formatFloat,
	type MeshDetail,
	type MeshInfo,
	shortShader,
	type TextureDetail,
} from "@/components/studio/inspect";
import {
	CHANNEL_LABEL,
	CHANNELS,
	type Channel,
	channelView,
	drawThumbnail,
	type Pixels,
	pixelAt,
	readTexturePixels,
	usedChannels,
} from "@/components/studio/texture-preview";
import {
	GlassCard,
	IconButton,
	PanelHeading,
} from "@/components/studio/ui/chrome";
import { Badge } from "@/components/ui/badge";
import {
	Dialog,
	DialogDescription,
	DialogHeader,
	DialogPanel,
	DialogPopup,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

const ALPHA_LABEL = { opaque: "Opaque", mask: "Cutout", blend: "Blend" };

/** Checkerboard behind transparent textures. */
const checker =
	"bg-[length:16px_16px] bg-[linear-gradient(45deg,var(--color-muted)_25%,transparent_25%,transparent_75%,var(--color-muted)_75%),linear-gradient(45deg,var(--color-muted)_25%,transparent_25%,transparent_75%,var(--color-muted)_75%)] [background-position:0_0,8px_8px]";

function Rows({
	rows,
}: {
	rows: [string, React.ReactNode][];
}): React.ReactElement {
	return (
		<dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
			{rows.map(([k, v]) => (
				<div className="contents" key={k}>
					<dt className="text-muted-foreground">{k}</dt>
					<dd className="min-w-0 truncate text-end tabular-nums">{v}</dd>
				</div>
			))}
		</dl>
	);
}

function Chips({ items }: { items: string[] }): React.ReactElement {
	return (
		<div className="flex flex-wrap justify-end gap-1">
			{items.map((item) => (
				<Badge
					className="font-mono text-[0.625rem]"
					key={item}
					size="sm"
					variant="outline"
				>
					{item}
				</Badge>
			))}
		</div>
	);
}

// ------------------------------------------------------------- textures

function TextureThumb({
	texture,
	size,
}: {
	texture: Texture;
	size: number;
}): React.ReactElement {
	const ref = React.useRef<HTMLCanvasElement>(null);
	const [ok, setOk] = React.useState(true);
	React.useEffect(() => {
		const canvas = ref.current;
		if (!canvas) return;
		setOk(drawThumbnail(texture, canvas, size));
	}, [texture, size]);
	return (
		<span
			className={cn(
				"relative flex shrink-0 items-center justify-center overflow-hidden rounded-md border bg-background",
				checker,
			)}
			style={{ width: size, height: size }}
		>
			<canvas className={cn("size-full", !ok && "hidden")} ref={ref} />
			{!ok && (
				<ImageIcon
					aria-hidden="true"
					className="size-4 text-muted-foreground"
				/>
			)}
		</span>
	);
}

function TextureTile({
	slot,
	onOpen,
}: {
	slot: TextureDetail;
	onOpen: (slot: TextureDetail) => void;
}): React.ReactElement {
	const name = slot.slot.replace(/^_/, "");
	const size =
		slot.width && slot.height ? `${slot.width}×${slot.height}` : null;
	return (
		<button
			className="flex min-w-0 flex-col items-center gap-1 rounded-lg p-1.5 text-center transition-colors hover:bg-accent/60 focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
			disabled={!slot.texture}
			onClick={() => onOpen(slot)}
			title={
				slot.texture
					? `${slot.slot}${slot.name ? ` · ${slot.name}` : ""} · open preview`
					: `${slot.slot}: not embedded`
			}
			type="button"
		>
			{slot.texture ? (
				<TextureThumb size={56} texture={slot.texture} />
			) : (
				<span className="flex size-14 items-center justify-center rounded-md border border-dashed text-muted-foreground">
					<ImageIcon aria-hidden="true" className="size-4" />
				</span>
			)}
			<span className="w-full truncate font-mono text-[0.6875rem]">{name}</span>
			<span className="w-full truncate text-[0.625rem] text-muted-foreground tabular-nums">
				{size ?? "—"}
				{slot.colorSpace && ` · ${slot.colorSpace}`}
			</span>
		</button>
	);
}

/**
 * Full-size view of one texture with per-channel display, for reading data
 * maps (ramps, SDFs, packed masks) that mean nothing as colour.
 */
export function TexturePreviewDialog({
	slot,
	onClose,
}: {
	slot: TextureDetail | null;
	onClose: () => void;
}): React.ReactElement {
	const [channel, setChannel] = React.useState<Channel>("rgb");
	const [hover, setHover] = React.useState<{
		x: number;
		y: number;
		rgba: [number, number, number, number];
	} | null>(null);
	// State, not a ref: the dialog mounts its popup after `open` flips, so
	// the draw effect must re-run once the canvas exists.
	const [canvas, setCanvas] = React.useState<HTMLCanvasElement | null>(null);
	const texture = slot?.texture ?? null;
	const pixels = React.useMemo<Pixels | null>(
		() => (texture ? readTexturePixels(texture) : null),
		[texture],
	);
	const used = React.useMemo(
		() => (pixels ? usedChannels(pixels.data) : new Set<Channel>()),
		[pixels],
	);
	// New texture: back to RGB, unless it is a single-channel data map.
	React.useEffect(() => {
		if (!pixels) return;
		const single = used.size === 1 && !used.has("a") ? [...used][0] : undefined;
		setChannel(single ?? "rgb");
		setHover(null);
	}, [pixels, used]);
	React.useEffect(() => {
		if (!canvas || !pixels) return;
		canvas.width = pixels.width;
		canvas.height = pixels.height;
		const ctx = canvas.getContext("2d");
		ctx?.putImageData(
			new ImageData(
				channelView(pixels.data, channel),
				pixels.width,
				pixels.height,
			),
			0,
			0,
		);
	}, [canvas, pixels, channel]);
	const onMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
		if (!pixels) return;
		const rect = e.currentTarget.getBoundingClientRect();
		const x = ((e.clientX - rect.left) / rect.width) * pixels.width;
		const y = ((e.clientY - rect.top) / rect.height) * pixels.height;
		const rgba = pixelAt(pixels, x, y);
		setHover(rgba ? { x: Math.floor(x), y: Math.floor(y), rgba } : null);
	};
	const small = pixels ? Math.max(pixels.width, pixels.height) <= 256 : false;
	// 1-D ramps and LUT strips (256×1, 1024×32) get stretched so they read.
	const strip = pixels ? pixels.height * 8 < pixels.width : false;
	return (
		<Dialog onOpenChange={(open) => !open && onClose()} open={Boolean(slot)}>
			<DialogPopup className="max-w-3xl">
				<DialogHeader>
					<DialogTitle className="font-mono">{slot?.slot}</DialogTitle>
					<DialogDescription>
						{slot?.name ?? "Embedded texture"}
						{slot?.width && slot.height && ` · ${slot.width}×${slot.height}`}
						{slot?.colorSpace && ` · ${slot.colorSpace}`}
						{slot?.uvSet !== null &&
							slot?.uvSet !== undefined &&
							` · UV${slot.uvSet}`}
					</DialogDescription>
				</DialogHeader>
				<DialogPanel className="flex flex-col gap-3">
					<div className="flex flex-wrap items-center justify-between gap-2">
						<ToggleGroup
							aria-label="Channel"
							onValueChange={(v) => {
								const next = v[0] as Channel | undefined;
								if (next) setChannel(next);
							}}
							size="sm"
							value={[channel]}
							variant="outline"
						>
							{CHANNELS.map((c) => (
								<ToggleGroupItem
									className={cn(
										c !== "rgb" && !used.has(c) && "text-muted-foreground/60",
									)}
									key={c}
									title={
										c === "rgb"
											? "Colour channels"
											: used.has(c)
												? `${CHANNEL_LABEL[c]} channel`
												: `${CHANNEL_LABEL[c]} channel (constant)`
									}
									value={c}
								>
									{CHANNEL_LABEL[c]}
								</ToggleGroupItem>
							))}
						</ToggleGroup>
						<span className="min-w-0 font-mono text-muted-foreground text-xs tabular-nums">
							{hover
								? `${hover.x}, ${hover.y}  ·  ${hover.rgba.join("  ")}`
								: pixels
									? "Hover for pixel values"
									: ""}
						</span>
					</div>
					<div
						className={cn(
							"flex max-h-[60dvh] items-center justify-center overflow-hidden rounded-lg border",
							checker,
						)}
					>
						{pixels ? (
							<canvas
								className={cn(
									"max-h-[60dvh] max-w-full",
									strip ? "object-fill" : "object-contain",
									(small || strip) && "[image-rendering:pixelated]",
								)}
								onPointerLeave={() => setHover(null)}
								onPointerMove={onMove}
								ref={setCanvas}
								style={
									strip
										? { width: "100%", height: 96 }
										: small
											? {
													width: Math.min(pixels.width * 2, 512),
													height: Math.min(pixels.height * 2, 512),
												}
											: undefined
								}
							/>
						) : (
							<p className="p-8 text-center text-muted-foreground text-sm">
								This texture's pixels can't be read back (compressed or
								unsupported format).
							</p>
						)}
					</div>
				</DialogPanel>
			</DialogPopup>
		</Dialog>
	);
}

// ------------------------------------------------------------- card

function Swatch({ color }: { color: ColorDetail }): React.ReactElement {
	return (
		<span className="flex min-w-0 items-center justify-end gap-1.5">
			<span
				aria-hidden="true"
				className="size-3 shrink-0 rounded-sm border border-border/70"
				style={{ background: color.hex }}
			/>
			<span
				className="truncate font-mono text-[0.6875rem]"
				title={color.rgba.map(formatFloat).join(", ")}
			>
				{color.hdr
					? color.rgba.slice(0, 3).map(formatFloat).join(" ")
					: color.hex}
				{color.rgba[3] !== 1 && (
					<span className="text-muted-foreground">
						{" "}
						α{formatFloat(color.rgba[3])}
					</span>
				)}
			</span>
		</span>
	);
}

const matches = (name: string, q: string) =>
	!q || name.toLowerCase().includes(q);

export interface InspectorCardProps {
	info: MeshInfo;
	detail: MeshDetail;
	canIsolate: boolean;
	focused: boolean;
	onFocus: () => void;
	onIsolateFocus: () => void;
	onHide: () => void;
	onClose: () => void;
	className?: string;
}

/** Expanded detail of the picked mesh, below its selection chip. */
export function InspectorCard({
	info,
	detail,
	canIsolate,
	focused,
	onFocus,
	onIsolateFocus,
	onHide,
	onClose,
	className,
}: InspectorCardProps): React.ReactElement {
	const [query, setQuery] = React.useState("");
	const [preview, setPreview] = React.useState<TextureDetail | null>(null);
	const q = query.trim().toLowerCase();
	const { geometry: g, material: m } = detail;
	const floats = m.floats.filter(([n]) => matches(n, q));
	const ints = m.ints.filter(([n]) => matches(n, q));
	const colors = m.colors.filter((c) => matches(c.name, q));
	const propertyCount = m.floats.length + m.ints.length + m.colors.length;
	return (
		<GlassCard
			aria-label="Mesh details"
			className={cn(
				"flex w-[min(22rem,calc(100vw-1.5rem))] flex-col gap-3 overflow-y-auto p-3 pt-2 [scrollbar-width:thin]",
				className,
			)}
			role="region"
		>
			<div className="flex items-center justify-between gap-2">
				<div className="flex min-w-0 flex-col">
					<span className="truncate font-medium text-sm" title={info.name}>
						{info.name}
					</span>
					{info.path.length > 0 && (
						<span className="truncate font-mono text-[0.625rem] text-muted-foreground">
							{info.path.join(" / ")}
						</span>
					)}
				</div>
				<div className="flex shrink-0 items-center gap-0.5">
					<IconButton
						aria-pressed={focused}
						className={cn(focused && "bg-primary/12 text-primary")}
						label="Focus the camera on this mesh"
						onClick={onFocus}
						shortcut="."
					>
						<CrosshairIcon />
					</IconButton>
					<IconButton
						disabled={!canIsolate}
						label="Isolate and focus"
						onClick={onIsolateFocus}
						shortcut="/"
					>
						<ScanSearchIcon />
					</IconButton>
					<IconButton
						disabled={!canIsolate}
						label="Hide this part"
						onClick={onHide}
						shortcut="⌫"
					>
						<EyeOffIcon />
					</IconButton>
					<IconButton label="Close details" onClick={onClose}>
						<XIcon />
					</IconButton>
				</div>
			</div>

			<section className="flex flex-col gap-1.5">
				<PanelHeading>Geometry</PanelHeading>
				<Rows
					rows={[
						["Vertices", formatNumber(g.vertices)],
						[
							"Triangles",
							`${formatNumber(g.triangles)}${g.indexed ? "" : " (unindexed)"}`,
						],
						["Submeshes", String(g.submeshes)],
						[
							"Skinning",
							g.skinned ? `${formatNumber(g.bones ?? 0)} bones` : "Static",
						],
						["UV sets", g.uvSets.length ? g.uvSets.join(", ") : "None"],
						[
							"Attributes",
							<Chips
								items={g.attributes.map((a) => `${a.name}·${a.size}`)}
								key="attrs"
							/>,
						],
					]}
				/>
			</section>

			<section className="flex flex-col gap-1.5">
				<PanelHeading>Material</PanelHeading>
				<Rows
					rows={[
						[
							"Shader",
							m.shader ? (
								<Badge
									className="max-w-full font-mono"
									key="shader"
									size="sm"
									title={m.shader}
									variant="secondary"
								>
									<span className="truncate">{shortShader(m.shader)}</span>
								</Badge>
							) : (
								"Standard PBR"
							),
						],
						[
							"Name",
							<span className="font-mono" key="name" title={m.name}>
								{m.name}
								{info.materialCount > 1 &&
									` (${info.materialIndex + 1}/${info.materialCount})`}
							</span>,
						],
						[
							"Alpha",
							m.alphaMode === "mask"
								? `${ALPHA_LABEL.mask} · cutoff ${formatFloat(m.alphaCutoff ?? 0.5)}`
								: ALPHA_LABEL[m.alphaMode],
						],
						["Sides", m.doubleSided ? "Double" : "Front"],
						...(m.renderQueue !== null
							? ([["Queue", String(m.renderQueue)]] as [string, string][])
							: []),
						...(m.keywords.length
							? ([["Keywords", <Chips items={m.keywords} key="kw" />]] as [
									string,
									React.ReactNode,
								][])
							: []),
					]}
				/>
				{propertyCount > 0 && (
					<div className="flex flex-col gap-1.5 pt-1">
						<Input
							aria-label="Filter material properties"
							className="font-mono"
							onChange={(e) => setQuery(e.target.value)}
							placeholder={`Filter ${propertyCount} properties`}
							size="sm"
							type="search"
							value={query}
						/>
						<dl className="grid max-h-56 grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 overflow-y-auto pe-1 text-xs [scrollbar-width:thin]">
							{colors.map((c) => (
								<div className="contents" key={c.name}>
									<dt
										className="truncate font-mono text-[0.6875rem] text-muted-foreground"
										title={c.name}
									>
										{c.name}
									</dt>
									<dd className="min-w-0">
										<Swatch color={c} />
									</dd>
								</div>
							))}
							{floats.map(([name, value]) => (
								<div className="contents" key={name}>
									<dt
										className="truncate font-mono text-[0.6875rem] text-muted-foreground"
										title={name}
									>
										{name}
									</dt>
									<dd className="text-end font-mono text-[0.6875rem] tabular-nums">
										{formatFloat(value)}
									</dd>
								</div>
							))}
							{ints.map(([name, value]) => (
								<div className="contents" key={name}>
									<dt
										className="truncate font-mono text-[0.6875rem] text-muted-foreground"
										title={name}
									>
										{name}
									</dt>
									<dd className="text-end font-mono text-[0.6875rem] tabular-nums">
										{value}
									</dd>
								</div>
							))}
							{!floats.length && !ints.length && !colors.length && (
								<p className="col-span-2 flex items-center gap-1.5 py-1 text-muted-foreground">
									<SearchIcon aria-hidden="true" className="size-3" />
									Nothing matches “{query}”.
								</p>
							)}
						</dl>
					</div>
				)}
			</section>

			<section className="flex flex-col gap-1.5">
				<PanelHeading>
					Textures{detail.textures.length ? ` · ${detail.textures.length}` : ""}
				</PanelHeading>
				{detail.textures.length ? (
					<div className="-mx-1 grid grid-cols-3 gap-0.5">
						{detail.textures.map((slot) => (
							<TextureTile key={slot.slot} onOpen={setPreview} slot={slot} />
						))}
					</div>
				) : (
					<p className="text-muted-foreground text-xs">No textures.</p>
				)}
				<p className="text-[0.6875rem] text-muted-foreground">
					Click a texture to view it per channel.{" "}
					<Kbd className="h-4 min-w-4 text-[0.625rem]">Esc</Kbd> clears the
					selection.
				</p>
			</section>

			<TexturePreviewDialog onClose={() => setPreview(null)} slot={preview} />
		</GlassCard>
	);
}
