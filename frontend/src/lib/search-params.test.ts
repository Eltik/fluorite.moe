import { describe, expect, test } from "bun:test";
import {
	bool,
	nat,
	oneOf,
	searchParams,
	str,
	text,
	withDefault,
} from "@/lib/search-params";

describe("str", () => {
	test("keeps strings, including empty", () => {
		expect(str()("abc")).toBe("abc");
		expect(str()("")).toBe("");
	});
	test("stringifies finite numbers and booleans", () => {
		expect(str()(211120)).toBe("211120");
		expect(str()(1.5)).toBe("1.5");
		expect(str()(true)).toBe("true");
		expect(str()(false)).toBe("false");
	});
	test("rejects non-finite numbers and other types", () => {
		expect(str()(Number.NaN)).toBeUndefined();
		expect(str()(Number.POSITIVE_INFINITY)).toBeUndefined();
		expect(str()(null)).toBeUndefined();
		expect(str()(undefined)).toBeUndefined();
		expect(str()({})).toBeUndefined();
		expect(str()(["a"])).toBeUndefined();
	});
	test("drops values longer than max, keeps those at max", () => {
		expect(str(3)("abc")).toBe("abc");
		expect(str(3)("abcd")).toBeUndefined();
		expect(str(3)(12345)).toBeUndefined();
		expect(str(4)(true)).toBe("true");
		expect(str(3)(true)).toBeUndefined();
	});
});

describe("text", () => {
	test("accepts strings and finite numbers", () => {
		expect(text("hi")).toBe("hi");
		expect(text(130)).toBe("130");
	});
	test("rejects everything else, including booleans", () => {
		expect(text(true)).toBeUndefined();
		expect(text(Number.NaN)).toBeUndefined();
		expect(text(null)).toBeUndefined();
		expect(text(undefined)).toBeUndefined();
		expect(text({})).toBeUndefined();
	});
});

describe("oneOf", () => {
	const parse = oneOf(["a", "b"] as const);
	test("accepts listed options", () => {
		expect(parse("a")).toBe("a");
		expect(parse("b")).toBe("b");
	});
	test("rejects unlisted values and non-strings", () => {
		expect(parse("c")).toBeUndefined();
		expect(parse("")).toBeUndefined();
		expect(parse(1)).toBeUndefined();
		expect(parse(undefined)).toBeUndefined();
	});
});

describe("bool", () => {
	test("only real booleans pass", () => {
		expect(bool(true)).toBe(true);
		expect(bool(false)).toBe(false);
		expect(bool("true")).toBeUndefined();
		expect(bool(1)).toBeUndefined();
		expect(bool(undefined)).toBeUndefined();
	});
});

describe("nat", () => {
	test("accepts non-negative safe integers", () => {
		expect(nat(0)).toBe(0);
		expect(nat(42)).toBe(42);
		expect(nat(Number.MAX_SAFE_INTEGER)).toBe(Number.MAX_SAFE_INTEGER);
	});
	test("rejects negatives, fractions, unsafe, NaN and non-numbers", () => {
		expect(nat(-1)).toBeUndefined();
		expect(nat(1.5)).toBeUndefined();
		expect(nat(Number.MAX_SAFE_INTEGER + 1)).toBeUndefined();
		expect(nat(Number.NaN)).toBeUndefined();
		expect(nat(Number.POSITIVE_INFINITY)).toBeUndefined();
		expect(nat("3")).toBeUndefined();
	});
});

describe("withDefault", () => {
	const parse = withDefault(nat, 7);
	test("passes valid values through", () => {
		expect(parse(3)).toBe(3);
		expect(parse(0)).toBe(0);
	});
	test("falls back when invalid and exposes the fallback", () => {
		expect(parse("x")).toBe(7);
		expect(parse(undefined)).toBe(7);
		expect(parse.fallback).toBe(7);
	});
	test("keeps a valid falsy value rather than the fallback", () => {
		expect(withDefault(bool, true)(false)).toBe(false);
	});
});

describe("searchParams", () => {
	const validate = searchParams({
		q: str(5),
		page: withDefault(nat, 1),
		sort: oneOf(["asc", "desc"] as const),
		open: bool,
	});
	test("parses valid input", () => {
		expect(validate({ q: "abc", page: 2, sort: "asc", open: true })).toEqual({
			q: "abc",
			page: 2,
			sort: "asc",
			open: true,
		});
	});
	test("drops invalid keys and applies defaults", () => {
		expect(
			validate({ q: "toolong", page: -3, sort: "up", open: "yes" }),
		).toEqual({ page: 1 });
	});
	test("ignores unknown keys and handles empty input", () => {
		expect(validate({ extra: 1 })).toEqual({ page: 1 });
		expect(validate({})).toEqual({ page: 1 });
	});
	test("does not emit keys for undefined results", () => {
		expect("q" in validate({})).toBe(false);
	});
});
