import { describe, expect, test } from "bun:test";
import {
	clampFov,
	distanceForFov,
	distanceFromZoom,
	nextSurface,
	presetDirection,
	VIEW_PRESET_IDS,
	VIEW_PRESET_KEYS,
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
