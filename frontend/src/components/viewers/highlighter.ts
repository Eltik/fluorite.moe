/**
 * Lazily created Shiki highlighter (JS regex engine, three grammars, two
 * themes). Only imported from the code viewer chunk.
 */
import type { HighlighterCore, ThemedToken } from "shiki/core";

export type HighlightLang = "lua" | "json" | "xml";

let highlighter: Promise<HighlighterCore> | null = null;

function getHighlighter(): Promise<HighlighterCore> {
	if (!highlighter) {
		highlighter = (async () => {
			const [{ createHighlighterCore }, { createJavaScriptRegexEngine }] =
				await Promise.all([
					import("shiki/core"),
					import("shiki/engine/javascript"),
				]);
			return createHighlighterCore({
				engine: createJavaScriptRegexEngine(),
				themes: [
					import("@shikijs/themes/github-light-default"),
					import("@shikijs/themes/github-dark-default"),
				],
				langs: [
					import("@shikijs/langs/lua"),
					import("@shikijs/langs/json"),
					import("@shikijs/langs/xml"),
				],
			});
		})();
		highlighter.catch(() => {
			highlighter = null;
		});
	}
	return highlighter;
}

export async function tokenize(
	code: string,
	lang: HighlightLang,
): Promise<ThemedToken[][]> {
	const hl = await getHighlighter();
	return hl.codeToTokens(code, {
		lang,
		themes: { light: "github-light-default", dark: "github-dark-default" },
		defaultColor: "light",
	}).tokens;
}
