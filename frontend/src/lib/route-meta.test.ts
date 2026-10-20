import { describe, expect, test } from "bun:test";
import { crumbLabel, pageTitle } from "@/lib/route-meta";

describe("route-meta", () => {
	test("crumbLabel resolves strings and functions", () => {
		expect(crumbLabel("Home", {})).toBe("Home");
		expect(crumbLabel((p, d) => `${p.id}-${String(d)}`, { id: "7" }, "x")).toBe(
			"7-x",
		);
	});
	test("pageTitle skips empty parts and appends the brand", () => {
		expect(pageTitle("A", "", "B")).toBe("A · B · fluorite");
		expect(pageTitle()).toBe("fluorite");
	});
});
