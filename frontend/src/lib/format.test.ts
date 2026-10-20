import { describe, expect, test } from "bun:test";
import {
	basename,
	dirname,
	extname,
	formatBytes,
	formatCompact,
	formatDate,
	formatDateTime,
	formatNumber,
	formatRelative,
	previewKind,
} from "@/lib/format";

describe("formatNumber", () => {
	test("groups thousands (en-US)", () => {
		expect(formatNumber(0)).toBe("0");
		expect(formatNumber(1234567)).toBe("1,234,567");
		expect(formatNumber(-1234)).toBe("-1,234");
		expect(formatNumber(1234.5)).toBe("1,234.5");
	});
});

describe("formatBytes", () => {
	test("placeholder for missing or NaN", () => {
		expect(formatBytes(undefined)).toBe("—");
		expect(formatBytes(Number.NaN)).toBe("—");
	});
	test("bytes below 1 KB", () => {
		expect(formatBytes(0)).toBe("0 B");
		expect(formatBytes(1023)).toBe("1023 B");
	});
	test("scales units with one decimal under 10", () => {
		expect(formatBytes(1024)).toBe("1.0 KB");
		expect(formatBytes(1536)).toBe("1.5 KB");
		expect(formatBytes(10 * 1024)).toBe("10 KB");
		expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
		expect(formatBytes(5 * 1024 ** 3)).toBe("5.0 GB");
	});
	test("caps at TB", () => {
		expect(formatBytes(2048 * 1024 ** 4)).toBe("2048 TB");
	});
});

describe("formatRelative", () => {
	const now = Date.parse("2026-10-20T12:00:00Z");
	test("returns the input for unparseable dates", () => {
		expect(formatRelative("nope", now)).toBe("nope");
	});
	test("just now under a minute", () => {
		expect(formatRelative("2026-10-20T11:59:30Z", now)).toBe("just now");
	});
	test("past and future units", () => {
		expect(formatRelative("2026-10-20T11:55:00Z", now)).toBe("5 minutes ago");
		expect(formatRelative("2026-10-20T09:00:00Z", now)).toBe("3 hours ago");
		expect(formatRelative("2026-10-18T12:00:00Z", now)).toBe("2 days ago");
		expect(formatRelative("2026-10-20T14:00:00Z", now)).toBe("in 2 hours");
		expect(formatRelative("2026-09-20T12:00:00Z", now)).toBe("last month");
		expect(formatRelative("2024-10-20T12:00:00Z", now)).toBe("2 years ago");
	});
});

describe("formatDateTime / formatDate", () => {
	test("deterministic UTC output", () => {
		expect(formatDateTime("2026-10-20T08:35:00Z")).toBe(
			"Oct 20, 2026, 08:35 UTC",
		);
		expect(formatDateTime("2026-01-05T00:07:00Z")).toBe(
			"Jan 5, 2026, 00:07 UTC",
		);
		expect(formatDate("2026-10-20T23:59:00Z")).toBe("Oct 20, 2026");
	});
	test("invalid input is returned unchanged", () => {
		expect(formatDateTime("bad")).toBe("bad");
		expect(formatDate("bad")).toBe("bad");
	});
});

describe("formatCompact", () => {
	test("short counts", () => {
		expect(formatCompact(340)).toBe("340");
		expect(formatCompact(999)).toBe("999");
		expect(formatCompact(1000)).toBe("1k");
		expect(formatCompact(1500)).toBe("1.5k");
		expect(formatCompact(73_000)).toBe("73k");
		expect(formatCompact(2_100_000)).toBe("2.1M");
		expect(formatCompact(3_000_000)).toBe("3M");
	});
});

describe("path helpers", () => {
	test("extname", () => {
		expect(extname("a/b/file.PNG")).toBe("png");
		expect(extname("archive.tar.gz")).toBe("gz");
		expect(extname("noext")).toBe("");
		expect(extname(".hidden")).toBe("");
	});
	test("dirname / basename", () => {
		expect(dirname("a/b/c.txt")).toBe("a/b");
		expect(dirname("c.txt")).toBe("");
		expect(basename("a/b/c.txt")).toBe("c.txt");
		expect(basename("c.txt")).toBe("c.txt");
	});
	test("previewKind", () => {
		expect(previewKind("x.json")).toBe("json");
		expect(previewKind("x.PNG")).toBe("image");
		expect(previewKind("x.jpeg")).toBe("image");
		expect(previewKind("x.glb")).toBe("model");
		expect(previewKind("x.lua")).toBe("code");
		expect(previewKind("x.bin")).toBe("binary");
		expect(previewKind("noext")).toBe("binary");
	});
});
