import { describe, expect, test } from "bun:test";
import { parseJsonSafe } from "@/lib/json-parse";

describe("parseJsonSafe", () => {
	test("behaves like JSON.parse for ordinary input", () => {
		expect(parseJsonSafe('{"a":[1,2,{"b":null}],"c":"x"}')).toEqual({
			a: [1, 2, { b: null }],
			c: "x",
		});
		expect(parseJsonSafe("123")).toBe(123);
	});
	test("keeps unsafe integers exact as strings", () => {
		expect(parseJsonSafe('{"id": 8724881275565959844}')).toEqual({
			id: "8724881275565959844",
		});
		expect(parseJsonSafe("[-8724881275565959844, 1]")).toEqual([
			"-8724881275565959844",
			1,
		]);
	});
	test("leaves safe 16-digit integers numeric", () => {
		expect(parseJsonSafe('{"n": 9007199254740991}')).toEqual({
			n: 9007199254740991,
		});
	});
	test("does not touch digits inside strings", () => {
		expect(
			parseJsonSafe('{"s":"8724881275565959844","n":8724881275565959844}'),
		).toEqual({ s: "8724881275565959844", n: "8724881275565959844" });
		expect(parseJsonSafe('{"s":"a\\"8724881275565959844"}')).toEqual({
			s: 'a"8724881275565959844',
		});
	});
	test("does not rewrite floats or exponents", () => {
		expect(parseJsonSafe('{"f":1234567890123456.5}')).toEqual({
			f: 1234567890123456.5,
		});
		expect(parseJsonSafe('{"e":12345678901234567e2}')).toEqual({
			e: JSON.parse("12345678901234567e2"),
		});
	});
	test("still throws on invalid JSON", () => {
		expect(() => parseJsonSafe("{nope")).toThrow();
	});
});
