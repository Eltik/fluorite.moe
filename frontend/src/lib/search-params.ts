/**
 * Tiny route search-param parsers. Each parser returns the cleaned value, or
 * `undefined` when the input is missing or invalid, so bad URLs degrade to
 * "param not set" instead of throwing. Used in place of zod so the validators
 * on every route's critical path stay out of the initial JS.
 */

type Parser<T> = (value: unknown) => T | undefined;

/** Any string, optionally capped in length (longer values are dropped). */
export const str =
	(max = Infinity): Parser<string> =>
	(v) =>
		typeof v === "string" && v.length <= max ? v : undefined;

/**
 * Optional string search param. The router JSON-parses values, so `?q=130`
 * or `?line=17` arrive as numbers; accept both and keep strings.
 */
export const text: Parser<string> = (v) =>
	typeof v === "string"
		? v
		: typeof v === "number" && Number.isFinite(v)
			? String(v)
			: undefined;

export const oneOf =
	<const T extends readonly string[]>(options: T): Parser<T[number]> =>
	(v) =>
		typeof v === "string" && (options as readonly string[]).includes(v)
			? (v as T[number])
			: undefined;

export const bool: Parser<boolean> = (v) =>
	typeof v === "boolean" ? v : undefined;

/** Non-negative integer. */
export const nat: Parser<number> = (v) =>
	typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : undefined;

/** Parser that falls back to `fallback` instead of leaving the key unset. */
export const withDefault = <T>(
	parser: Parser<T>,
	fallback: T,
): Parser<T> & { fallback: T } =>
	Object.assign((v: unknown) => parser(v) ?? fallback, { fallback });

type Shape = Record<string, Parser<unknown>>;
type Parsed<S extends Shape> = {
	[K in keyof S as S[K] extends { fallback: unknown } ? K : never]: NonNullable<
		ReturnType<S[K]>
	>;
} & {
	[K in keyof S as S[K] extends { fallback: unknown }
		? never
		: K]?: NonNullable<ReturnType<S[K]>>;
};

/** Builds a `validateSearch` function from per-key parsers; invalid keys are left out. */
export function searchParams<S extends Shape>(
	shape: S,
): (input: Record<string, unknown>) => Parsed<S> {
	return (input) => {
		const out: Record<string, unknown> = {};
		for (const key in shape) {
			const value = shape[key](input[key]);
			if (value !== undefined) out[key] = value;
		}
		return out as Parsed<S>;
	};
}
