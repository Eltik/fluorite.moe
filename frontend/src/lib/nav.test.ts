import { describe, expect, test } from "bun:test";
import { fileTarget } from "@/lib/nav";

describe("fileTarget", () => {
	test("top-level table JSON opens the table viewer", () => {
		expect(fileTarget("tables", "Foo.json")).toEqual({
			to: "/tables/$name",
			params: { name: "Foo" },
		});
	});
	test("nested table files and other roots open the browser", () => {
		expect(fileTarget("tables", "sub/Foo.json")).toEqual({
			to: "/browse/$root",
			params: { root: "tables" },
			search: { path: "sub", file: "sub/Foo.json" },
		});
		expect(fileTarget("textures", "a/b.png")).toEqual({
			to: "/browse/$root",
			params: { root: "textures" },
			search: { path: "a", file: "a/b.png" },
		});
	});
});
