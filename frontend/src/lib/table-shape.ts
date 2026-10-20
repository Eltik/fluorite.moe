/**
 * Turns game-table JSON into rows for the data grid, when it has a tabular
 * shape. Supported shapes:
 *   - `[{...}, {...}]`                      array of records
 *   - `{ id: {...}, id2: {...} }`           dictionary of records (`_key` column)
 *   - `{ id: { array: [{...}] }, ... }`     dictionary of record lists
 *   - `{ only: [{...}] }`                   single wrapper key around a list
 */

export type Rec = Record<string, unknown>;

export interface Tabular {
	rows: Rec[];
	columns: string[];
	/** Name of the synthetic key column, if one was added. */
	keyColumn: string | null;
}

export const KEY_COLUMN = "_key";
const SAMPLE = 2_000;
const MAX_COLUMNS = 400;

function isRecord(value: unknown): value is Rec {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function mostlyRecords(values: unknown[]): boolean {
	if (values.length === 0) return false;
	const sample = values.slice(0, SAMPLE);
	return sample.filter(isRecord).length / sample.length >= 0.9;
}

function collectColumns(rows: Rec[], keyColumn: string | null): string[] {
	const seen = new Set<string>();
	if (keyColumn) seen.add(keyColumn);
	for (const row of rows.slice(0, SAMPLE)) {
		for (const key of Object.keys(row)) {
			seen.add(key);
			if (seen.size >= MAX_COLUMNS) break;
		}
	}
	return [...seen];
}

export function toTabular(value: unknown): Tabular | null {
	if (Array.isArray(value)) {
		if (!mostlyRecords(value)) return null;
		const rows = value.filter(isRecord);
		return { rows, columns: collectColumns(rows, null), keyColumn: null };
	}
	if (!isRecord(value)) return null;
	const entries = Object.entries(value);
	if (entries.length === 0) return null;

	if (entries.length === 1) {
		const [, only] = entries[0] as [string, unknown];
		if (Array.isArray(only)) return toTabular(only);
	}

	const values = entries.map(([, v]) => v);
	if (!mostlyRecords(values)) return null;

	// Dictionary of `{ array: [...] }` wrappers: flatten the inner records.
	const wrapped = values
		.slice(0, SAMPLE)
		.every(
			(v) =>
				isRecord(v) &&
				Object.keys(v).length === 1 &&
				Array.isArray((v as Rec).array),
		);
	if (wrapped) {
		const rows: Rec[] = [];
		for (const [key, v] of entries) {
			const list = (v as { array: unknown[] }).array;
			for (const item of list) {
				rows.push(
					isRecord(item)
						? { [KEY_COLUMN]: key, ...item }
						: { [KEY_COLUMN]: key, value: item },
				);
			}
		}
		if (rows.length === 0) return null;
		return {
			rows,
			columns: collectColumns(rows, KEY_COLUMN),
			keyColumn: KEY_COLUMN,
		};
	}

	// A config object with a handful of heterogeneous records is not a table.
	if (entries.length < 2) return null;
	const rows = entries
		.filter(([, v]) => isRecord(v))
		.map(([key, v]) => ({ [KEY_COLUMN]: key, ...(v as Rec) }));
	return {
		rows,
		columns: collectColumns(rows, KEY_COLUMN),
		keyColumn: KEY_COLUMN,
	};
}

// ---------------------------------------------------------------- i18n

export interface I18nRef {
	id: string;
	text: string;
}

/** Game text references look like `{ id: <int64>, text: "" }`. */
export function asI18n(value: unknown): I18nRef | null {
	if (!isRecord(value)) return null;
	const keys = Object.keys(value);
	if (keys.length !== 2 || !("id" in value) || !("text" in value)) return null;
	const { id, text } = value;
	if (typeof text !== "string") return null;
	if (typeof id !== "string" && typeof id !== "number") return null;
	return { id: String(id), text };
}

/** Unresolved i18n ids (empty `text`, non-zero id) anywhere in the rows. */
export function collectI18nIds(rows: Rec[], limit = 50_000): string[] {
	const ids = new Set<string>();
	const visit = (value: unknown, depth: number) => {
		if (ids.size >= limit || depth > 4) return;
		const ref = asI18n(value);
		if (ref) {
			if (!ref.text && ref.id !== "0") ids.add(ref.id);
			return;
		}
		if (Array.isArray(value)) for (const v of value) visit(v, depth + 1);
		else if (isRecord(value))
			for (const v of Object.values(value)) visit(v, depth + 1);
	};
	for (const row of rows) for (const v of Object.values(row)) visit(v, 0);
	return [...ids];
}

/** Display text for a cell value (used for filtering and sorting). */
export function cellText(
	value: unknown,
	resolve?: (id: string) => string | undefined,
): string {
	if (value === null || value === undefined) return "";
	if (typeof value !== "object") return String(value);
	const ref = asI18n(value);
	if (ref) return ref.text || resolve?.(ref.id) || ref.id;
	if (
		Array.isArray(value) &&
		value.every((v) => typeof v !== "object" || v === null)
	) {
		return value.join(", ");
	}
	const json = JSON.stringify(value);
	return json.length > 300 ? `${json.slice(0, 300)}…` : json;
}
