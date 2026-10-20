import { describe, expect, test } from "bun:test";
import {
	asI18n,
	cellText,
	collectI18nIds,
	KEY_COLUMN,
	toTabular,
} from "@/lib/table-shape";

describe("toTabular", () => {
	test("array of records", () => {
		const t = toTabular([{ a: 1 }, { a: 2, b: 3 }]);
		expect(t).toEqual({
			rows: [{ a: 1 }, { a: 2, b: 3 }],
			columns: ["a", "b"],
			keyColumn: null,
		});
	});
	test("array tolerates a few non-records (>= 90%) and drops them", () => {
		const items = Array.from({ length: 9 }, (_, i) => ({ i }));
		const t = toTabular([...items, 5]);
		expect(t?.rows).toHaveLength(9);
	});
	test("mostly non-record arrays and empty arrays are not tables", () => {
		expect(toTabular([1, 2, 3])).toBeNull();
		expect(toTabular([{ a: 1 }, 2])).toBeNull();
		expect(toTabular([])).toBeNull();
	});
	test("scalars, null and empty objects are not tables", () => {
		expect(toTabular(null)).toBeNull();
		expect(toTabular("x")).toBeNull();
		expect(toTabular(5)).toBeNull();
		expect(toTabular({})).toBeNull();
	});
	test("dictionary of records adds a key column first", () => {
		const t = toTabular({ one: { x: 1 }, two: { x: 2, y: 3 } });
		expect(t?.keyColumn).toBe(KEY_COLUMN);
		expect(t?.columns).toEqual([KEY_COLUMN, "x", "y"]);
		expect(t?.rows).toEqual([
			{ _key: "one", x: 1 },
			{ _key: "two", x: 2, y: 3 },
		]);
	});
	test("dictionary of { array } wrappers is flattened", () => {
		const t = toTabular({
			a: { array: [{ v: 1 }, { v: 2 }] },
			b: { array: [{ v: 3 }, 9] },
		});
		expect(t?.rows).toEqual([
			{ _key: "a", v: 1 },
			{ _key: "a", v: 2 },
			{ _key: "b", v: 3 },
			{ _key: "b", value: 9 },
		]);
		expect(t?.columns).toEqual([KEY_COLUMN, "v", "value"]);
	});
	test("wrappers with no inner rows are not tables", () => {
		expect(toTabular({ a: { array: [] }, b: { array: [] } })).toBeNull();
	});
	test("single wrapper key around a list unwraps", () => {
		const t = toTabular({ only: [{ a: 1 }] });
		expect(t).toEqual({ rows: [{ a: 1 }], columns: ["a"], keyColumn: null });
	});
	test("single key holding a record is not a table", () => {
		expect(toTabular({ cfg: { a: 1 } })).toBeNull();
	});
	test("heterogeneous config object is not a table", () => {
		expect(toTabular({ a: 1, b: "x", c: true })).toBeNull();
	});
	test("a record key named _key is overridden by the real key", () => {
		const t = toTabular({ one: { x: 1 }, two: { x: 2 } });
		expect(t?.rows[0]?._key).toBe("one");
	});
	test("caps columns at 400", () => {
		const wide: Record<string, number> = {};
		for (let i = 0; i < 600; i++) wide[`c${i}`] = i;
		const t = toTabular([wide]);
		expect(t?.columns.length).toBe(400);
	});
});

describe("asI18n", () => {
	test("matches { id, text } with string or numeric id", () => {
		expect(asI18n({ id: "123", text: "hi" })).toEqual({
			id: "123",
			text: "hi",
		});
		expect(asI18n({ id: 5, text: "" })).toEqual({ id: "5", text: "" });
	});
	test("rejects other shapes", () => {
		expect(asI18n({ id: 1, text: "a", extra: 1 })).toBeNull();
		expect(asI18n({ id: 1 })).toBeNull();
		expect(asI18n({ id: 1, text: 2 })).toBeNull();
		expect(asI18n({ id: true, text: "" })).toBeNull();
		expect(asI18n([])).toBeNull();
		expect(asI18n(null)).toBeNull();
		expect(asI18n("x")).toBeNull();
	});
});

describe("collectI18nIds", () => {
	test("collects unresolved ids only", () => {
		const rows = [
			{
				a: { id: "10", text: "" },
				b: { id: "11", text: "resolved" },
				c: { id: "0", text: "" },
			},
		];
		expect(collectI18nIds(rows)).toEqual(["10"]);
	});
	test("dedupes and finds nested refs in arrays and objects", () => {
		const rows = [
			{
				list: [
					{ id: "1", text: "" },
					{ id: "2", text: "" },
				],
			},
			{ deep: { x: { id: "1", text: "" } } },
		];
		expect(collectI18nIds(rows).sort()).toEqual(["1", "2"]);
	});
	test("stops descending past depth 4", () => {
		const nest = (depth: number, leaf: unknown): unknown => {
			let v = leaf;
			for (let i = 0; i < depth; i++) v = { k: v };
			return v;
		};
		const ref = { id: "9", text: "" };
		// row value is visited at depth 0; ref found at depth == wrappers count.
		expect(collectI18nIds([{ v: nest(4, ref) }])).toEqual(["9"]);
		expect(collectI18nIds([{ v: nest(5, ref) }])).toEqual([]);
	});
	test("respects the limit", () => {
		const rows = Array.from({ length: 10 }, (_, i) => ({
			r: { id: String(i + 1), text: "" },
		}));
		expect(collectI18nIds(rows, 3)).toHaveLength(3);
	});
});

describe("cellText", () => {
	test("nullish and primitives", () => {
		expect(cellText(null)).toBe("");
		expect(cellText(undefined)).toBe("");
		expect(cellText(5)).toBe("5");
		expect(cellText(false)).toBe("false");
	});
	test("i18n refs use text, then resolver, then id", () => {
		expect(cellText({ id: "1", text: "Hello" })).toBe("Hello");
		expect(cellText({ id: "1", text: "" }, () => "Resolved")).toBe("Resolved");
		expect(cellText({ id: "1", text: "" }, () => undefined)).toBe("1");
		expect(cellText({ id: "1", text: "" })).toBe("1");
	});
	test("primitive arrays join; others JSON", () => {
		expect(cellText([1, "a", null])).toBe("1, a, ");
		expect(cellText({ a: 1 })).toBe('{"a":1}');
		expect(cellText([{ a: 1 }])).toBe('[{"a":1}]');
	});
	test("long JSON is truncated at 300 chars with an ellipsis", () => {
		const out = cellText({ s: "x".repeat(500) });
		expect(out.length).toBe(301);
		expect(out.endsWith("…")).toBe(true);
	});
});
