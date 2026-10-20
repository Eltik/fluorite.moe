import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
	BoxesIcon,
	CornerDownLeftIcon,
	FileIcon,
	SearchIcon,
} from "lucide-react";
import * as React from "react";
import { NAV } from "@/components/shell/nav-config";
import {
	Command,
	CommandCollection,
	CommandDialog,
	CommandDialogPopup,
	CommandEmpty,
	CommandFooter,
	CommandGroup,
	CommandGroupLabel,
	CommandInput,
	CommandItem,
	CommandList,
	CommandPanel,
} from "@/components/ui/command";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { useDebouncedValue } from "@/hooks/use-debounced-value";
import { isApiError, type Root, type SearchItem, thumbUrl } from "@/lib/api";
import {
	type CatalogHit,
	catalogSearchQuery,
	KIND_ORDER,
	KIND_SINGULAR,
} from "@/lib/catalog";
import { dirname, formatBytes } from "@/lib/format";
import { fileTarget, ROOT_META } from "@/lib/nav";
import { searchQuery } from "@/lib/queries";
import { useSettings } from "@/lib/settings";
import { isStudioKind, studioLink } from "@/lib/studio";

// ---------------------------------------------------------------- context

interface PaletteContext {
	open: boolean;
	setOpen: (open: boolean) => void;
}

const Context = React.createContext<PaletteContext | null>(null);

export function useCommandPalette(): PaletteContext {
	const ctx = React.useContext(Context);
	if (!ctx) throw new Error("useCommandPalette needs <CommandPaletteProvider>");
	return ctx;
}

export function CommandPaletteProvider({
	children,
}: {
	children: React.ReactNode;
}): React.ReactElement {
	const [open, setOpen] = React.useState(false);

	React.useEffect(() => {
		const onKey = (event: KeyboardEvent) => {
			if (event.key.toLowerCase() === "k" && (event.metaKey || event.ctrlKey)) {
				event.preventDefault();
				setOpen((value) => !value);
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, []);

	const value = React.useMemo(() => ({ open, setOpen }), [open]);

	return (
		<Context.Provider value={value}>
			{children}
			<CommandDialog onOpenChange={setOpen} open={open}>
				<CommandDialogPopup aria-label="Search fluorite">
					{open && <PaletteBody onDone={() => setOpen(false)} />}
				</CommandDialogPopup>
			</CommandDialog>
		</Context.Provider>
	);
}

// ---------------------------------------------------------------- body

type PageEntry = {
	type: "page";
	id: string;
	label: string;
	section: string;
	nav: (typeof NAV)[number]["items"][number];
};
type FileEntry = { type: "file"; id: string; item: SearchItem };
type EntityEntry = { type: "entity"; id: string; hit: CatalogHit };
type Entry = PageEntry | FileEntry | EntityEntry;
type Group = { value: string; label: string; items: Entry[] };

const PAGES: PageEntry[] = NAV.flatMap((section) =>
	section.items.map((nav) => ({
		type: "page" as const,
		id: `page:${nav.href}`,
		label: nav.label,
		section: section.label ?? "General",
		nav,
	})),
);

/** Files shown for plain-word queries; path-like queries get more, per root. */
const MAX_FILES_PLAIN = 6;
const MAX_PER_ROOT = 8;
const MAX_ENTITIES = 10;

/** Path-like queries (`ui/sprites`, `.png`, `chr_0005`) are about files. */
function looksLikePath(q: string): boolean {
	return /[/._]/.test(q);
}

/**
 * Lower is better: exact name, then prefix, then word prefix, then substring.
 * Shared by entities (name) and files (basename without extension).
 */
function nameScore(name: string, needle: string): number {
	const n = name.toLowerCase();
	if (n === needle) return 0;
	if (n.startsWith(needle)) return 1;
	if (n.split(/[^\p{L}\p{N}]+/u).some((word) => word.startsWith(needle)))
		return 2;
	if (n.includes(needle)) return 3;
	return 4;
}

function rankEntities(hits: CatalogHit[], needle: string): CatalogHit[] {
	const scored = hits.map((hit) => ({
		hit,
		score: Math.min(
			nameScore(hit.name, needle),
			// Ids like `chr_0005_chen` still count, one notch below the name.
			nameScore(hit.id, needle) + 1,
		),
		kind: KIND_ORDER.indexOf(hit.kind),
	}));
	// Same-name entries of one kind (NPC variants) collapse into the first.
	const seen = new Set<string>();
	const unique = scored.filter(({ hit }) => {
		const key = `${hit.kind}:${hit.name}`;
		if (seen.has(key)) return false;
		seen.add(key);
		return true;
	});
	unique.sort(
		(a, b) =>
			a.score - b.score ||
			a.kind - b.kind ||
			(b.hit.rarity ?? 0) - (a.hit.rarity ?? 0) ||
			a.hit.name.length - b.hit.name.length,
	);
	return unique.map((s) => s.hit);
}

function rankFiles(items: SearchItem[], needle: string): SearchItem[] {
	const scored = items.map((item) => {
		const stem = item.name.replace(/\.[^.]+$/, "");
		const base = nameScore(stem, needle);
		// Matches only in the directory part rank below any filename match.
		const score =
			base < 4 ? base : item.path.toLowerCase().includes(needle) ? 5 : 6;
		return { item, score };
	});
	scored.sort(
		(a, b) => a.score - b.score || a.item.path.length - b.item.path.length,
	);
	return scored.map((s) => s.item);
}

function PaletteBody({ onDone }: { onDone: () => void }): React.ReactElement {
	const navigate = useNavigate();
	const [value, setValue] = React.useState("");
	const q = useDebouncedValue(value.trim(), 180);
	// Shift+Enter (or shift-click) opens a model-backed entity in the Studio.
	const shift = React.useRef(false);
	React.useEffect(() => {
		const track = (event: KeyboardEvent) => {
			shift.current = event.shiftKey;
		};
		window.addEventListener("keydown", track, true);
		window.addEventListener("keyup", track, true);
		return () => {
			window.removeEventListener("keydown", track, true);
			window.removeEventListener("keyup", track, true);
		};
	}, []);

	const search = useQuery({
		...searchQuery({ q, limit: 80 }),
		placeholderData: keepPreviousData,
	});
	const { textLanguage } = useSettings();
	const catalog = useQuery({
		...catalogSearchQuery(q, textLanguage),
		placeholderData: keepPreviousData,
	});

	const groups = React.useMemo<Group[]>(() => {
		const needle = value.trim().toLowerCase();
		const pages = PAGES.filter(
			(p) =>
				!needle ||
				p.label.toLowerCase().includes(needle) ||
				p.section.toLowerCase().includes(needle),
		).sort((a, b) =>
			needle ? nameScore(a.label, needle) - nameScore(b.label, needle) : 0,
		);
		const pageGroup: Group | null = pages.length
			? { value: "pages", label: "Go to", items: pages }
			: null;
		if (!needle || !q) return pageGroup ? [pageGroup] : [];

		const out: Group[] = [];
		const hits = catalog.data?.flatMap((group) => group.items) ?? [];
		if (hits.length > 0) {
			const total = catalog.data?.reduce((n, g) => n + g.total, 0) ?? 0;
			const ranked = rankEntities(hits, needle).slice(0, MAX_ENTITIES);
			out.push({
				value: "catalog",
				label: `Catalog · ${total.toLocaleString("en-US")}`,
				items: ranked.map((hit) => ({
					type: "entity" as const,
					id: `entity:${hit.kind}:${hit.id}`,
					hit,
				})),
			});
		}
		// Pages that merely contain the word stay below catalog hits.
		if (pageGroup) out.push(pageGroup);

		if (search.data && search.data.items.length > 0) {
			const ranked = rankFiles(search.data.items, needle);
			if (looksLikePath(needle)) {
				const byRoot = new Map<Root, FileEntry[]>();
				for (const item of ranked) {
					const list = byRoot.get(item.root) ?? [];
					if (list.length < MAX_PER_ROOT) {
						list.push({ type: "file", id: `${item.root}:${item.path}`, item });
					}
					byRoot.set(item.root, list);
				}
				for (const [root, items] of byRoot) {
					out.push({ value: root, label: ROOT_META[root].label, items });
				}
			} else {
				out.push({
					value: "files",
					label: "Game files",
					items: ranked.slice(0, MAX_FILES_PLAIN).map((item) => ({
						type: "file" as const,
						id: `${item.root}:${item.path}`,
						item,
					})),
				});
			}
		}
		return out;
	}, [value, q, search.data, catalog.data]);

	const open = (entry: Entry) => {
		onDone();
		if (entry.type === "page") {
			void navigate(entry.nav.link);
		} else if (entry.type === "entity") {
			const { kind, id, hasModel } = entry.hit;
			if (shift.current && hasModel && isStudioKind(kind)) {
				void navigate(studioLink(kind, id));
			} else {
				void navigate({ to: "/catalog/$kind/$id", params: { kind, id } });
			}
		} else {
			void navigate(fileTarget(entry.item.root, entry.item.path));
		}
	};

	const pending = value.trim() !== q || search.isFetching || catalog.isFetching;
	const status = (() => {
		if (!value.trim())
			return "Search operators, items, enemies… or any file path";
		if (search.isError) {
			return isApiError(search.error) && search.error.status === 0
				? "Search is unavailable: the API is offline"
				: "Search failed";
		}
		if (pending && !search.data) return "Searching…";
		if (search.data && q) {
			const shown = groups
				.filter((g) => g.value !== "pages" && g.value !== "catalog")
				.reduce((n, g) => n + g.items.length, 0);
			const more =
				search.data.total > shown && !looksLikePath(q.toLowerCase())
					? " · add / or .ext to list more"
					: "";
			return `${search.data.total.toLocaleString("en-US")} file matches${
				search.data.total > shown ? ` · showing ${shown}` : ""
			}${more}`;
		}
		return "";
	})();

	return (
		<Command
			filter={null}
			itemToStringValue={(value) => {
				const entry = value as Entry;
				return entry.type === "page"
					? entry.label
					: entry.type === "entity"
						? entry.hit.name
						: entry.item.path;
			}}
			items={groups}
			onValueChange={setValue}
			value={value}
		>
			<CommandInput
				aria-label="Search pages, catalog and files"
				placeholder="Search pages, catalog and files…"
			/>
			<CommandPanel>
				<CommandEmpty>
					<div className="flex flex-col items-center gap-2 text-muted-foreground text-sm">
						<SearchIcon className="size-4 opacity-60" />
						{pending ? "Searching…" : `No results for “${value.trim()}”`}
					</div>
				</CommandEmpty>
				<CommandList>
					{(group: Group) => (
						<CommandGroup items={group.items} key={group.value}>
							<CommandGroupLabel>{group.label}</CommandGroupLabel>
							<CommandCollection>
								{(entry: Entry) => (
									<CommandItem
										className="gap-2.5"
										key={entry.id}
										onClick={() => open(entry)}
										value={entry}
									>
										<EntryRow entry={entry} />
									</CommandItem>
								)}
							</CommandCollection>
						</CommandGroup>
					)}
				</CommandList>
			</CommandPanel>
			<CommandFooter>
				<span
					aria-live="polite"
					className="flex min-w-0 items-center gap-2 truncate"
				>
					{pending && value.trim() ? <Spinner className="size-3" /> : null}
					{status}
				</span>
				<span className="hidden items-center gap-3 sm:flex">
					<span className="flex items-center gap-1.5">
						<KbdGroup>
							<Kbd>↑</Kbd>
							<Kbd>↓</Kbd>
						</KbdGroup>
						Navigate
					</span>
					<span className="flex items-center gap-1.5">
						<Kbd>
							<CornerDownLeftIcon />
						</Kbd>
						Open
					</span>
				</span>
			</CommandFooter>
		</Command>
	);
}

function EntryRow({ entry }: { entry: Entry }): React.ReactElement {
	if (entry.type === "page") {
		const Icon = entry.nav.icon;
		return (
			<>
				<Icon className="size-4 text-muted-foreground" />
				<span className="flex-1 truncate">{entry.label}</span>
				<span className="text-muted-foreground text-xs">{entry.section}</span>
			</>
		);
	}
	if (entry.type === "entity") {
		const { hit } = entry;
		return (
			<>
				{hit.icon ? (
					<img
						alt=""
						aria-hidden="true"
						className="size-6 shrink-0 rounded-md bg-muted object-contain"
						loading="lazy"
						src={thumbUrl(hit.icon, 128)}
					/>
				) : (
					<span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted font-medium text-[0.625rem] text-muted-foreground">
						{[...hit.name][0]}
					</span>
				)}
				<span className="flex-1 truncate">{hit.name}</span>
				{hit.hasModel && isStudioKind(hit.kind) && (
					<span
						className="flex shrink-0 items-center gap-1 text-muted-foreground text-xs opacity-0 in-data-highlighted:opacity-100"
						title="Shift+Enter opens this model in the Studio"
					>
						<Kbd>⇧↵</Kbd>
						<BoxesIcon className="size-3.5" />
						Studio
					</span>
				)}
				<span className="shrink-0 text-muted-foreground text-xs">
					{KIND_SINGULAR[hit.kind]}
				</span>
			</>
		);
	}
	const { item } = entry;
	const Icon = ROOT_META[item.root]?.icon ?? FileIcon;
	const dir = dirname(item.path);
	return (
		<>
			<Icon className="size-4 shrink-0 text-muted-foreground" />
			<span className="flex min-w-0 flex-1 flex-col">
				<span className="truncate">{item.name}</span>
				{dir && (
					<span className="truncate text-muted-foreground text-xs" dir="rtl">
						<bdi>{dir}</bdi>
					</span>
				)}
			</span>
			<span className="shrink-0 text-muted-foreground text-xs tabular-nums">
				{formatBytes(item.size)}
			</span>
		</>
	);
}
