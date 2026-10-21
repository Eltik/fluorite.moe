import { describe, expect, test } from "bun:test";
import {
	clampFov,
	distanceForFov,
	distanceFromZoom,
	distanceLimits,
	dollyTargetTowards,
	nearPlaneFor,
	nextSurface,
	presetDirection,
	VIEW_PRESET_IDS,
	VIEW_PRESET_KEYS,
	wheelZoomFactor,
	zoomFromDistance,
} from "@/components/studio/view-math";

const length = (v: number[]) => Math.hypot(v[0], v[1], v[2]);

describe("zoom mapping", () => {
	test("is 0 at the far limit and 1 at the near limit", () => {
		expect(zoomFromDistance(10, 1, 10)).toBe(0);
		expect(zoomFromDistance(1, 1, 10)).toBe(1);
	});

	test("is logarithmic: the geometric middle is halfway", () => {
		expect(zoomFromDistance(Math.sqrt(10), 1, 10)).toBeCloseTo(0.5, 10);
	});

	test("clamps distances outside the range", () => {
		expect(zoomFromDistance(0.1, 1, 10)).toBe(1);
		expect(zoomFromDistance(100, 1, 10)).toBe(0);
	});

	test("round-trips through distanceFromZoom", () => {
		for (const d of [0.5, 1, 2.5, 7, 20]) {
			const t = zoomFromDistance(d, 0.5, 20);
			expect(distanceFromZoom(t, 0.5, 20)).toBeCloseTo(d, 9);
		}
	});

	test("survives degenerate limits", () => {
		expect(zoomFromDistance(5, 0, 0)).toBe(0.5);
		expect(zoomFromDistance(Number.NaN, 1, 10)).toBe(0.5);
		expect(distanceFromZoom(0.5, 0, 0)).toBe(1);
		expect(distanceFromZoom(2, 1, 10)).toBe(1);
		expect(distanceFromZoom(-1, 1, 10)).toBe(10);
	});
});

describe("distance limits", () => {
	test("a 1 m figure can be approached to 2 cm and seen from 14 m", () => {
		const { min, max } = distanceLimits(1);
		expect(min).toBeCloseTo(0.02, 9);
		expect(max).toBeCloseTo(14, 9);
	});

	test("scale with the model but never closer than 2 cm", () => {
		expect(distanceLimits(5).min).toBeCloseTo(0.1, 9);
		expect(distanceLimits(0.3).min).toBeCloseTo(0.02, 9);
		expect(distanceLimits(0).max).toBeGreaterThan(distanceLimits(0).min);
	});

	test("the slider spans the whole range on a log scale", () => {
		const { min, max } = distanceLimits(1);
		expect(zoomFromDistance(max, min, max)).toBe(0);
		expect(zoomFromDistance(min, min, max)).toBe(1);
		expect(zoomFromDistance(Math.sqrt(min * max), min, max)).toBeCloseTo(
			0.5,
			9,
		);
	});
});

describe("near plane", () => {
	test("follows the camera in for close-ups", () => {
		expect(nearPlaneFor(0.02, 1)).toBeCloseTo(0.001, 9);
		expect(nearPlaneFor(0.3, 1)).toBeCloseTo(0.01, 9);
	});

	test("is capped so depth precision holds across the model", () => {
		expect(nearPlaneFor(100, 1)).toBeCloseTo(0.01, 9);
		expect(nearPlaneFor(0.0001, 1)).toBeCloseTo(0.0005, 9);
	});

	test("never drops below a hair for tiny models", () => {
		expect(nearPlaneFor(0.001, 0.01)).toBeGreaterThanOrEqual(1e-4);
	});
});

describe("wheel zoom", () => {
	const wheel = (deltaY: number, ctrlKey = false, deltaMode = 0) =>
		wheelZoomFactor({ deltaY, deltaMode, ctrlKey });

	test("scrolling up zooms in, down zooms out, symmetrically", () => {
		expect(wheel(-100)).toBeLessThan(1);
		expect(wheel(100)).toBeGreaterThan(1);
		expect(wheel(-100) * wheel(100)).toBeCloseTo(1, 9);
	});

	test("one mouse notch is a clear step: about 0.62×", () => {
		expect(wheel(-100)).toBeCloseTo(0.62, 2);
	});

	test("small trackpad deltas integrate to the same curve", () => {
		let d = 1;
		for (let i = 0; i < 25; i++) d *= wheel(-4);
		expect(d).toBeCloseTo(wheel(-100), 9);
	});

	test("a trackpad pinch gets extra gain", () => {
		expect(wheel(-10, true)).toBeLessThan(wheel(-10));
		// A doubling pinch (~70 px of deltas) is a clear 2–3× dolly; a long
		// one (~150 px) goes from full body to a bust.
		let d = 1;
		for (let i = 0; i < 14; i++) d *= wheel(-5, true);
		expect(d).toBeGreaterThan(0.3);
		expect(d).toBeLessThan(0.45);
		for (let i = 0; i < 16; i++) d *= wheel(-5, true);
		expect(d).toBeLessThan(0.15);
	});

	test("line and page deltas are scaled to pixels", () => {
		expect(wheel(-3, false, 1)).toBeCloseTo(wheel(-48), 9);
		expect(wheel(-1, false, 2)).toBeCloseTo(wheel(-400), 9);
	});

	test("one event is capped, and junk is ignored", () => {
		expect(wheel(-5000)).toBeCloseTo(wheel(-400), 9);
		expect(wheel(0)).toBe(1);
		expect(wheel(Number.NaN)).toBe(1);
	});

	test("the full 1 m range takes a handful of swipes", () => {
		const { min, max } = distanceLimits(1);
		const notches = Math.log(max / min) / -Math.log(wheel(-100));
		expect(notches).toBeLessThan(16);
		expect(notches).toBeGreaterThan(8);
	});
});

describe("dolly towards the cursor", () => {
	test("halving the distance moves the target halfway to the cursor point", () => {
		expect(dollyTargetTowards([0, 0, 0], [2, 0, 4], 0.5)).toEqual([1, 0, 2]);
	});

	test("zooming out moves the target away from the cursor point", () => {
		const t = dollyTargetTowards([0, 1, 0], [1, 1, 0], 2);
		expect(t[0]).toBeCloseTo(-1, 9);
		expect(t[1]).toBe(1);
	});

	test("is the identity for an unchanged distance", () => {
		expect(dollyTargetTowards([3, 2, 1], [9, 9, 9], 1)).toEqual([3, 2, 1]);
	});
});

describe("view presets", () => {
	test("every preset is a unit vector", () => {
		for (const id of VIEW_PRESET_IDS) {
			expect(length(presetDirection(id))).toBeCloseTo(1, 9);
		}
	});

	test("front and back face along Z, left and right along X", () => {
		expect(presetDirection("front")).toEqual([0, 0, 1]);
		expect(presetDirection("back")).toEqual([0, 0, -1]);
		expect(presetDirection("left")).toEqual([-1, 0, 0]);
		expect(presetDirection("right")).toEqual([1, 0, 0]);
	});

	test("top looks down with a defined up vector", () => {
		const top = presetDirection("top");
		expect(top[1]).toBeGreaterThan(0.999);
		expect(Math.hypot(top[0], top[2])).toBeGreaterThan(0);
	});

	test("three-quarter is above the horizon, front-right", () => {
		const d = presetDirection("threeQuarter");
		expect(d[0]).toBeGreaterThan(0);
		expect(d[1]).toBeGreaterThan(0);
		expect(d[2]).toBeGreaterThan(d[0]);
	});

	test("digits 1–6 map onto every preset once", () => {
		const mapped = Object.values(VIEW_PRESET_KEYS);
		expect(new Set(mapped).size).toBe(VIEW_PRESET_IDS.length);
		for (const id of VIEW_PRESET_IDS) expect(mapped).toContain(id);
	});
});

describe("dolly zoom", () => {
	test("a wider lens moves the camera closer to hold the subject size", () => {
		const d = distanceForFov(10, 20, 40);
		expect(d).toBeLessThan(10);
		// The visible height 2·d·tan(fov/2) is unchanged.
		expect(2 * d * Math.tan((40 * Math.PI) / 360)).toBeCloseTo(
			2 * 10 * Math.tan((20 * Math.PI) / 360),
			9,
		);
	});

	test("is the identity for the same field of view", () => {
		expect(distanceForFov(3.2, 20, 20)).toBeCloseTo(3.2, 12);
	});

	test("clampFov keeps the lens in range", () => {
		expect(clampFov(5)).toBe(10);
		expect(clampFov(120)).toBe(90);
		expect(clampFov(35)).toBe(35);
	});
});

describe("nextSurface", () => {
	test("cycles and wraps", () => {
		const ids = ["a", "b", "c"] as const;
		expect(nextSurface(ids, "a")).toBe("b");
		expect(nextSurface(ids, "c")).toBe("a");
	});

	test("starts over for an unknown value", () => {
		expect(nextSurface(["a", "b"], "zzz")).toBe("a");
	});
});
