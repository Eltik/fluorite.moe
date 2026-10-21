import { describe, expect, test } from "bun:test";
import {
	channelView,
	pixelAt,
	usedChannels,
} from "@/components/studio/texture-preview";

// Two pixels: a red one with half alpha, a blue one fully transparent.
const rgba = new Uint8ClampedArray([255, 0, 0, 128, 0, 0, 255, 0]);

describe("channelView", () => {
	test("RGB keeps the colour and forces alpha opaque", () => {
		expect([...channelView(rgba, "rgb")]).toEqual([
			255, 0, 0, 255, 0, 0, 255, 255,
		]);
	});

	test("a single channel becomes opaque greyscale", () => {
		expect([...channelView(rgba, "r")]).toEqual([
			255, 255, 255, 255, 0, 0, 0, 255,
		]);
		expect([...channelView(rgba, "b")]).toEqual([
			0, 0, 0, 255, 255, 255, 255, 255,
		]);
	});

	test("alpha is shown as a map of its own", () => {
		expect([...channelView(rgba, "a")]).toEqual([
			128, 128, 128, 255, 0, 0, 0, 255,
		]);
	});

	test("does not touch the source", () => {
		const copy = rgba.slice();
		channelView(rgba, "g");
		expect([...rgba]).toEqual([...copy]);
	});
});

describe("usedChannels", () => {
	test("reports channels that vary across the image", () => {
		expect([...usedChannels(rgba)].sort()).toEqual(["a", "b", "r"]);
	});

	test("a constant image uses nothing", () => {
		const flat = new Uint8ClampedArray([7, 7, 7, 255, 7, 7, 7, 255]);
		expect(usedChannels(flat).size).toBe(0);
	});
});

describe("pixelAt", () => {
	const pixels = { width: 2, height: 1, data: rgba };

	test("reads the pixel under a point, flooring fractions", () => {
		expect(pixelAt(pixels, 0.9, 0.2)).toEqual([255, 0, 0, 128]);
		expect(pixelAt(pixels, 1, 0)).toEqual([0, 0, 255, 0]);
	});

	test("is null outside the image", () => {
		expect(pixelAt(pixels, 2, 0)).toBeNull();
		expect(pixelAt(pixels, -1, 0)).toBeNull();
		expect(pixelAt(pixels, 0, 1)).toBeNull();
	});
});
