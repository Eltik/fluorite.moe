import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { CornerDownLeftIcon, FileIcon, SearchIcon } from "lucide-react";
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
import { isApiError, type Root, type SearchItem } from "@/lib/api";
import { dirname, formatBytes } from "@/lib/format";
import { fileTarget, ROOT_META } from "@/lib/nav";
import { searchQuery } from "@/lib/queries";

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
type Entry = PageEntry | FileEntry;
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

const MAX_PER_ROOT = 8;

function PaletteBody({ onDone }: { onDone: () => void }): React.ReactElement {
	const navigate = useNavigate();
	const [value, setValue] = React.useState("");
	const q = useDebouncedValue(value.trim(), 180);

	const search = useQuery({
		...searchQuery({ q, limit: 80 }),
		placeholderData: keepPreviousData,
	});

	const groups = React.useMemo<Group[]>(() => {
		const needle = value.trim().toLowerCase();
		const pages = PAGES.filter(
			(p) =>
				!needle ||
				p.label.toLowerCase().includes(needle) ||
				p.section.toLowerCase().includes(needle),
		);
		const out: Group[] = [];
		if (pages.length)
			out.push({ value: "pages", label: "Go to", items: pages });
		if (q && search.data) {
			const byRoot = new Map<Root, FileEntry[]>();
			for (const item of search.data.items) {
				const list = byRoot.get(item.root) ?? [];
				if (list.length < MAX_PER_ROOT) {
					list.push({ type: "file", id: `${item.root}:${item.path}`, item });
				}
				byRoot.set(item.root, list);
			}
			for (const [root, items] of byRoot) {
				out.push({ value: root, label: ROOT_META[root].label, items });
			}
		}
		return out;
	}, [value, q, search.data]);

	const open = (entry: Entry) => {
		onDone();
		if (entry.type === "page") {
			void navigate(entry.nav.link);
		} else {
			void navigate(fileTarget(entry.item.root, entry.item.path));
		}
	};

	const pending = value.trim() !== q || search.isFetching;
	const status = (() => {
		if (!value.trim()) return "Type to search every file path";
		if (search.isError) {
			return isApiError(search.error) && search.error.status === 0
				? "Search is unavailable: the API is offline"
				: "Search failed";
		}
		if (pending && !search.data) return "Searching…";
		if (search.data && q) {
			const shown = groups
				.filter((g) => g.value !== "pages")
				.reduce((n, g) => n + g.items.length, 0);
			return `${search.data.total.toLocaleString()} matches${
				search.data.total > shown ? ` · showing ${shown}` : ""
			}`;
		}
		return "";
	})();

	return (
		<Command
			filter={null}
			itemToStringValue={(value) => {
				const entry = value as Entry;
				return entry.type === "page" ? entry.label : entry.item.path;
			}}
			items={groups}
			onValueChange={setValue}
			value={value}
		>
			<CommandInput
				aria-label="Search pages and files"
				placeholder="Search pages and files…"
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
