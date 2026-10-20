/** Backdrop and lighting presets for the Studio stage. */
import {
	type BackgroundId,
	isHexColor,
	type LightingId,
} from "@/components/studio/prefs";

export interface ResolvedBackground {
	id: Exclude<BackgroundId, "auto">;
	/** CSS `background` drawn behind the transparent canvas. */
	css: string;
	/** Whether the backdrop is dark (drives grid / shadow contrast). */
	dark: boolean;
	/** Ground disc tint and centre opacity. */
	floor: { color: string; opacity: number };
	/** Paints the same backdrop into a 2D canvas (screenshots). */
	paint: (ctx: CanvasRenderingContext2D, w: number, h: number) => void;
}

interface BackgroundDef {
	label: string;
	/** Swatch preview CSS. */
	swatch: string;
}

export const BACKGROUNDS: Record<BackgroundId, BackgroundDef> = {
	auto: {
		label: "Theme",
		swatch:
			"linear-gradient(135deg, #f4f3f8 0%, #f4f3f8 50%, #1d1b26 50%, #1d1b26 100%)",
	},
	"stage-light": {
		label: "Light stage",
		swatch: "radial-gradient(circle at 50% 35%, #ffffff, #dcdbe6 70%, #c3c1d1)",
	},
	"stage-dark": {
		label: "Dark stage",
		swatch: "radial-gradient(circle at 50% 35%, #3b3650, #1b1925 70%, #0d0c12)",
	},
	black: { label: "Black", swatch: "#000000" },
	white: { label: "White", swatch: "#ffffff" },
	grey: { label: "Grey", swatch: "#7f7f86" },
	custom: {
		label: "Custom",
		swatch:
			"conic-gradient(from 90deg, #f472b6, #a78bfa, #60a5fa, #34d399, #fbbf24, #f472b6)",
	},
};

interface Stop {
	at: number;
	color: string;
}

function radial(stops: Stop[]): Pick<ResolvedBackground, "css" | "paint"> {
	return {
		css: `radial-gradient(120% 90% at 50% 38%, ${stops
			.map((s) => `${s.color} ${Math.round(s.at * 100)}%`)
			.join(", ")})`,
		paint: (ctx, w, h) => {
			// Approximate the elliptical CSS gradient by scaling a circular one.
			const rx = w * 1.2;
			const ry = h * 0.9;
			ctx.save();
			ctx.translate(w / 2, h * 0.38);
			ctx.scale(1, ry / rx);
			const g = ctx.createRadialGradient(0, 0, 0, 0, 0, rx);
			for (const s of stops) g.addColorStop(s.at, s.color);
			ctx.fillStyle = g;
			ctx.fillRect(-w, -h * (rx / ry), w * 2, h * 2 * (rx / ry));
			ctx.restore();
		},
	};
}

function solid(color: string): Pick<ResolvedBackground, "css" | "paint"> {
	return {
		css: color,
		paint: (ctx, w, h) => {
			ctx.fillStyle = color;
			ctx.fillRect(0, 0, w, h);
		},
	};
}

function luminance(hex: string): number {
	const n = Number.parseInt(hex.slice(1), 16);
	const r = (n >> 16) & 255;
	const g = (n >> 8) & 255;
	const b = n & 255;
	return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
}

const STAGE_LIGHT: Stop[] = [
	{ at: 0, color: "#ffffff" },
	{ at: 0.45, color: "#ebeaf2" },
	{ at: 1, color: "#c4c2d3" },
];
const STAGE_DARK: Stop[] = [
	{ at: 0, color: "#353048" },
	{ at: 0.5, color: "#1a1824" },
	{ at: 1, color: "#09080d" },
];

const LIGHT_FLOOR = { color: "#cfccdc", opacity: 0.7 };
const DARK_FLOOR = { color: "#3a3550", opacity: 0.55 };

/**
 * Resolves a background choice (`?bg=` overrides the saved preference) into
 * renderable CSS plus a screenshot painter.
 */
export function resolveBackground(
	choice: string | undefined,
	saved: BackgroundId,
	customColor: string,
	themeDark: boolean,
): ResolvedBackground {
	let id: BackgroundId = saved;
	let custom = customColor;
	if (choice && isHexColor(choice)) {
		id = "custom";
		custom = choice;
	} else if (choice && choice in BACKGROUNDS) {
		id = choice as BackgroundId;
	}
	if (id === "auto") id = themeDark ? "stage-dark" : "stage-light";
	switch (id) {
		case "stage-light":
			return { id, dark: false, floor: LIGHT_FLOOR, ...radial(STAGE_LIGHT) };
		case "stage-dark":
			return { id, dark: true, floor: DARK_FLOOR, ...radial(STAGE_DARK) };
		case "black":
			return {
				id,
				dark: true,
				floor: { color: "#2a2733", opacity: 0.55 },
				...solid("#000000"),
			};
		case "white":
			return {
				id,
				dark: false,
				floor: { color: "#d9d7e3", opacity: 0.6 },
				...solid("#ffffff"),
			};
		case "grey":
			return {
				id,
				dark: true,
				floor: { color: "#a4a3ad", opacity: 0.45 },
				...solid("#7f7f86"),
			};
		case "custom": {
			const dark = luminance(custom) < 0.5;
			return {
				id,
				dark,
				floor: dark
					? { color: "#ffffff", opacity: 0.08 }
					: { color: "#000000", opacity: 0.07 },
				...solid(custom),
			};
		}
	}
}

export interface DirLight {
	color: string;
	intensity: number;
	/** Direction the light comes from (normalised by the stage). */
	position: [number, number, number];
}

export interface Former {
	form: "rect" | "circle" | "ring";
	intensity: number;
	color?: string;
	position: [number, number, number];
	scale: [number, number, number];
	/** Lightformers face the origin; set false for a fixed rotation. */
	target?: boolean;
	rotation?: [number, number, number];
}

export interface LightingPreset {
	label: string;
	description: string;
	ambient: number;
	hemisphere: { sky: string; ground: string; intensity: number };
	key: DirLight;
	fill: DirLight;
	rim: DirLight;
	environment: number;
	formers: Former[];
	/** Light rig used with toon shading (no environment, no specular). */
	toon: ToonLights;
}

export interface ToonLights {
	ambient: number;
	hemisphere: { sky: string; ground: string; intensity: number };
	key: DirLight;
	/** Screen-space fresnel rim added in the toon shader. */
	rim: { color: string; strength: number };
}

export const LIGHTING: Record<LightingId, LightingPreset> = {
	studio: {
		label: "Studio",
		description: "Soft key, gentle fill and a cool rim",
		ambient: 0.15,
		hemisphere: { sky: "#ffffff", ground: "#5a5466", intensity: 0.55 },
		key: { color: "#fff4e8", intensity: 2.1, position: [3, 4.5, 3.5] },
		fill: { color: "#dfe6ff", intensity: 0.7, position: [-4, 2, 2] },
		rim: { color: "#c9b8ff", intensity: 1.6, position: [-1.5, 3, -4.5] },
		environment: 0.65,
		toon: {
			ambient: 0.25,
			hemisphere: { sky: "#fffaf4", ground: "#8f8aa8", intensity: 1 },
			key: { color: "#fff6ec", intensity: 2.3, position: [1.8, 3.6, 4.5] },
			rim: { color: "#efe9ff", strength: 0.3 },
		},
		formers: [
			{
				form: "rect",
				intensity: 2.5,
				position: [0, 6, 0],
				scale: [10, 10, 1],
				target: false,
				rotation: [Math.PI / 2, 0, 0],
			},
			{ form: "rect", intensity: 3, position: [4, 2, 4], scale: [3, 5, 1] },
			{
				form: "rect",
				intensity: 1.2,
				color: "#dfe6ff",
				position: [-5, 1, 2],
				scale: [3, 6, 1],
			},
			{
				form: "rect",
				intensity: 2,
				color: "#c9b8ff",
				position: [-2, 2, -6],
				scale: [8, 1.5, 1],
			},
		],
	},
	soft: {
		label: "Soft",
		description: "Even, low-contrast light for reading textures",
		ambient: 0.35,
		hemisphere: { sky: "#ffffff", ground: "#8a8596", intensity: 1 },
		key: { color: "#ffffff", intensity: 1.1, position: [2, 4, 4] },
		fill: { color: "#ffffff", intensity: 0.8, position: [-3, 2, 3] },
		rim: { color: "#ffffff", intensity: 0.5, position: [0, 3, -4] },
		environment: 1,
		toon: {
			ambient: 0.45,
			hemisphere: { sky: "#ffffff", ground: "#b4b0c2", intensity: 1.3 },
			key: { color: "#ffffff", intensity: 1.6, position: [1.5, 3, 5] },
			rim: { color: "#ffffff", strength: 0.12 },
		},
		formers: [
			{
				form: "rect",
				intensity: 1.5,
				position: [0, 6, 0],
				scale: [12, 12, 1],
				target: false,
				rotation: [Math.PI / 2, 0, 0],
			},
			{ form: "ring", intensity: 1.2, position: [0, 1, 6], scale: [5, 5, 1] },
			{ form: "rect", intensity: 1, position: [-6, 1, 0], scale: [4, 6, 1] },
			{ form: "rect", intensity: 1, position: [6, 1, 0], scale: [4, 6, 1] },
		],
	},
	dramatic: {
		label: "Dramatic",
		description: "Hard key with a bright violet rim",
		ambient: 0.04,
		hemisphere: { sky: "#b9b4ff", ground: "#120f1a", intensity: 0.2 },
		key: { color: "#fff0dc", intensity: 3, position: [4, 3, 2] },
		fill: { color: "#7d8bd6", intensity: 0.15, position: [-4, 1, 3] },
		rim: { color: "#a78bfa", intensity: 3.4, position: [-2.5, 2.5, -4] },
		environment: 0.3,
		toon: {
			ambient: 0.1,
			hemisphere: { sky: "#d9d4ff", ground: "#4a4266", intensity: 0.65 },
			key: { color: "#ffe9d2", intensity: 2.8, position: [4.5, 3, 1.5] },
			rim: { color: "#b79dff", strength: 0.75 },
		},
		formers: [
			{ form: "rect", intensity: 4, position: [5, 2, 2], scale: [2, 6, 1] },
			{
				form: "rect",
				intensity: 3,
				color: "#a78bfa",
				position: [-3, 2, -5],
				scale: [1, 6, 1],
			},
			{
				form: "rect",
				intensity: 3,
				color: "#a78bfa",
				position: [3, 2, -5],
				scale: [1, 6, 1],
			},
		],
	},
	flat: {
		label: "Flat",
		description: "Unshaded look: ambient light only",
		ambient: 1.8,
		hemisphere: { sky: "#ffffff", ground: "#ffffff", intensity: 0.6 },
		key: { color: "#ffffff", intensity: 0, position: [0, 1, 1] },
		fill: { color: "#ffffff", intensity: 0, position: [0, 1, 1] },
		rim: { color: "#ffffff", intensity: 0, position: [0, 1, -1] },
		environment: 0.2,
		toon: {
			ambient: 1.2,
			hemisphere: { sky: "#ffffff", ground: "#ffffff", intensity: 2 },
			key: { color: "#ffffff", intensity: 0, position: [0, 1, 1] },
			rim: { color: "#ffffff", strength: 0 },
		},
		formers: [
			{
				form: "rect",
				intensity: 1,
				position: [0, 6, 0],
				scale: [12, 12, 1],
				target: false,
				rotation: [Math.PI / 2, 0, 0],
			},
		],
	},
};
