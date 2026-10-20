/**
 * JSON.parse that keeps integers beyond Number.MAX_SAFE_INTEGER intact.
 *
 * Game tables use signed 64-bit i18n ids (e.g. `"id": 8724881275565959844`),
 * which plain JSON.parse silently rounds. Unsafe integer literals outside of
 * strings are rewritten into strings before parsing, so the id round-trips
 * exactly and can be sent to `/api/i18n/{lang}/resolve`.
 */
export function parseJsonSafe(text: string): unknown {
	if (!/\d{16}/.test(text)) return JSON.parse(text);
	return JSON.parse(quoteUnsafeIntegers(text));
}

const MAX_SAFE = String(Number.MAX_SAFE_INTEGER);

function isUnsafe(digits: string): boolean {
	if (digits.length !== MAX_SAFE.length) return digits.length > MAX_SAFE.length;
	return digits > MAX_SAFE;
}

function quoteUnsafeIntegers(text: string): string {
	const out: string[] = [];
	let last = 0;
	let i = 0;
	const n = text.length;
	while (i < n) {
		const c = text.charCodeAt(i);
		if (c === 34 /* " */) {
			i++;
			while (i < n) {
				const d = text.charCodeAt(i);
				if (d === 92 /* \ */) i += 2;
				else if (d === 34) {
					i++;
					break;
				} else i++;
			}
			continue;
		}
		if (c === 45 /* - */ || (c >= 48 && c <= 57)) {
			const start = i;
			if (c === 45) i++;
			const digitsStart = i;
			while (i < n) {
				const d = text.charCodeAt(i);
				if (d < 48 || d > 57) break;
				i++;
			}
			const next = text.charCodeAt(i);
			const isFloat = next === 46 || next === 101 || next === 69; // . e E
			if (isFloat) {
				// Skip the fraction/exponent so its digits are not re-scanned.
				while (i < n && /[0-9.eE+-]/.test(text[i] ?? "")) i++;
			} else if (isUnsafe(text.slice(digitsStart, i))) {
				out.push(text.slice(last, start), '"', text.slice(start, i), '"');
				last = i;
			}
			continue;
		}
		i++;
	}
	if (last === 0) return text;
	out.push(text.slice(last));
	return out.join("");
}
