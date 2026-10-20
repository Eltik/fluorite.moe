import { describe, expect, test } from "bun:test";
import { isStudioKind, STUDIO_KINDS, studioLink } from "@/lib/studio";

describe("studio", () => {
	test("isStudioKind", () => {
		for (const k of STUDIO_KINDS) expect(isStudioKind(k)).toBe(true);
		expect(isStudioKind("items")).toBe(false);
		expect(isStudioKind(undefined)).toBe(false);
	});
	test("studioLink", () => {
		expect(studioLink("operators", "x")).toEqual({
			to: "/studio/$kind/$id",
			params: { kind: "operators", id: "x" },
		});
	});
});
