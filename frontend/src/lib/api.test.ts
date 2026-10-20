import { describe, expect, test } from "bun:test";
import {
	API_URL,
	ApiError,
	apiConfigScript,
	encodePath,
	fileUrl,
	isApiError,
	isNetworkError,
	isRoot,
	modelFileUrl,
	ROOTS,
	thumbUrl,
} from "@/lib/api";

// src/test-setup.ts sets window.__FLUORITE_API_URL__ = "https://api.test/".

describe("API_URL", () => {
	test("comes from the window global with trailing slashes trimmed", () => {
		expect(API_URL).toBe("https://api.test");
	});
});

describe("encodePath", () => {
	test("encodes segments but keeps slashes", () => {
		expect(encodePath("a b/c#d/e?f")).toBe("a%20b/c%23d/e%3Ff");
	});
	test("drops empty segments (leading, trailing, doubled slashes)", () => {
		expect(encodePath("/a//b/")).toBe("a/b");
		expect(encodePath("")).toBe("");
	});
	test("encodes unicode", () => {
		expect(encodePath("角色/名前")).toBe(
			`${encodeURIComponent("角色")}/${encodeURIComponent("名前")}`,
		);
	});
});

describe("URL builders", () => {
	test("fileUrl", () => {
		expect(fileUrl("textures", "ui/a b.png")).toBe(
			"https://api.test/files/textures/ui/a%20b.png",
		);
	});
	test("modelFileUrl uses the models root", () => {
		expect(modelFileUrl("chr/x y.glb")).toBe(
			"https://api.test/files/models/chr/x%20y.glb",
		);
	});
	test("thumbUrl defaults to 256 and honours size", () => {
		expect(thumbUrl("a/b.png")).toBe("https://api.test/thumb/a/b.png?size=256");
		expect(thumbUrl("a/b.png", 512)).toBe(
			"https://api.test/thumb/a/b.png?size=512",
		);
	});
});

describe("apiConfigScript", () => {
	test("publishes API_URL as a JSON string", () => {
		expect(apiConfigScript()).toBe(
			'window.__FLUORITE_API_URL__="https://api.test";',
		);
	});
});

describe("isRoot", () => {
	test("accepts every root and rejects others", () => {
		for (const r of ROOTS) expect(isRoot(r)).toBe(true);
		expect(isRoot("models")).toBe(false);
		expect(isRoot("")).toBe(false);
	});
});

describe("errors", () => {
	test("ApiError flags", () => {
		const e = new ApiError(501, "not_implemented", "nope");
		expect(e.name).toBe("ApiError");
		expect(e.message).toBe("nope");
		expect(e.isNotImplemented).toBe(true);
		expect(e.isNotFound).toBe(false);
		expect(new ApiError(404, "x", "y").isNotFound).toBe(true);
	});
	test("isApiError", () => {
		expect(isApiError(new ApiError(500, "c", "m"))).toBe(true);
		expect(isApiError(new Error("m"))).toBe(false);
	});
	test("isNetworkError", () => {
		expect(isNetworkError(new TypeError("fetch failed"))).toBe(true);
		expect(isNetworkError(new ApiError(0, "unreachable", "m"))).toBe(true);
		expect(isNetworkError(new ApiError(500, "c", "m"))).toBe(false);
		expect(isNetworkError(new Error("m"))).toBe(false);
	});
});
