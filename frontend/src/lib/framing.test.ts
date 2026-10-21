import { describe, expect, test } from "bun:test";
import {
	cameraDirection,
	type FitInput,
	FRONT_DIRECTION,
	fitBox,
	type GameCamera,
	viewBasis,
} from "@/lib/framing";

const SI: GameCamera = {
	position: [0.177, 0.998, 3.5],
	target: [-0.022, 1.225, 0],
	fov: 20.0074,
	near: 0.1,
	far: 50,
	screenOffset: [-0.2489, 0.088],
	referenceAspect: 1.7778,
};

const NO_INSETS = { top: 0, bottom: 0, left: 0, right: 0 };

/** A standing character: 0.8 m wide (arms), 1.75 m tall, 0.4 m deep. */
const FIGURE = {
	min: [-0.4, 0, -0.2] as const,
	max: [0.4, 1.75, 0.2] as const,
};

function fit(partial: Partial<FitInput> = {}) {
	return fitBox({
		min: [...FIGURE.min],
		max: [...FIGURE.max],
		direction: [0, 0, 1],
		fov: 20,
		aspect: 16 / 9,
		insets: NO_INSETS,
		margin: 1,
		...partial,
	});
}

/** Vertical half-extent visible at `distance` for a 20° FOV. */
const halfVisible = (distance: number, fov = 20) =>
	distance * Math.tan((fov * Math.PI) / 360);

describe("cameraDirection / viewBasis", () => {
	test("points from the target to the camera, mostly along +Z for Si", () => {
		const d = cameraDirection(SI);
		expect(Math.hypot(...d)).toBeCloseTo(1, 6);
		expect(d[2]).toBeGreaterThan(0.99);
		// The game camera sits slightly below its target, so it looks up a bit.
		expect(d[1]).toBeLessThan(0);
	});

	test("the basis is orthonormal with the screen up roughly world up", () => {
		const { forward, right, up } = viewBasis(FRONT_DIRECTION);
		expect(forward[2]).toBeLessThan(0);
		expect(Math.abs(right[0] - 1)).toBeLessThan(1e-6);
		expect(up[1]).toBeGreaterThan(0.99);
	});
});

describe("fitBox", () => {
	test("the box is centred and exactly fills the free height on a wide screen", () => {
		const r = fit();
		expect(r.target[0]).toBeCloseTo(0, 6);
		expect(r.target[1]).toBeCloseTo(0.875, 6);
		expect(r.target[2]).toBeCloseTo(0, 6);
		// Height-limited: the half-height subtends half the FOV at the centre
		// plane, plus the half-depth push-back.
		const centreDistance = r.distance - 0.2;
		expect(halfVisible(centreDistance)).toBeCloseTo(0.875, 6);
		expect(r.position).toEqual([
			r.target[0],
			r.target[1],
			r.target[2] + r.distance,
		]);
	});

	test("portrait phones become width-limited and still show the whole box", () => {
		const r = fit({ aspect: 0.4 });
		const centreDistance = r.distance - 0.2;
		const halfW = halfVisible(centreDistance) * 0.4;
		expect(halfW).toBeCloseTo(0.4, 6);
		// Taller than needed for the height: nothing is cut off.
		expect(halfVisible(centreDistance)).toBeGreaterThan(0.875);
	});

	test("interface insets push the camera back and aim into the free area", () => {
		const plain = fit();
		const r = fit({ insets: { top: 0.1, bottom: 0.2, left: 0, right: 0 } });
		expect(r.distance).toBeGreaterThan(plain.distance);
		// Free area is 70 % of the height.
		expect(halfVisible(r.distance - 0.2) * 0.7).toBeCloseTo(0.875, 6);
		// More chrome at the bottom: the look-at point drops below the box
		// centre so the figure rises into the free area.
		expect(r.target[1]).toBeLessThan(0.875);
		const visibleH = 2 * r.distance * Math.tan((20 * Math.PI) / 360);
		expect(0.875 - r.target[1]).toBeCloseTo(0.05 * visibleH, 6);
	});

	test("margin scales the fit distance only", () => {
		const a = fit();
		const b = fit({ margin: 1.1 });
		expect(b.distance - 0.2).toBeCloseTo((a.distance - 0.2) * 1.1, 6);
	});

	test("a side view measures the depth as the width", () => {
		const wide = {
			min: [-0.4, 0, -1.5] as const,
			max: [0.4, 1.75, 1.5] as const,
		};
		const front = fit({ min: [...wide.min], max: [...wide.max] });
		const side = fit({
			min: [...wide.min],
			max: [...wide.max],
			direction: [1, 0, 0],
			aspect: 1,
		});
		// Square viewport, 3 m wide box from the side: width-limited.
		expect(halfVisible(side.distance - 0.4)).toBeCloseTo(1.5, 6);
		expect(side.distance).toBeGreaterThan(front.distance);
		expect(side.position[0]).toBeGreaterThan(side.target[0]);
	});

	test("uses the game camera's direction and fov", () => {
		const r = fit({ direction: cameraDirection(SI), fov: SI.fov });
		expect(r.position[2]).toBeGreaterThan(r.target[2]);
		expect(r.position[1]).toBeLessThan(r.target[1]);
		expect(r.position[0]).toBeGreaterThan(r.target[0]);
	});
});
