import * as React from "react";

const loadDialog = () => import("@/components/shell/command-palette-dialog");
const CommandPaletteDialog = React.lazy(loadDialog);

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

	// Warm the chunk once the page is idle so the first open is instant.
	React.useEffect(() => {
		const id = window.setTimeout(() => void loadDialog(), 2000);
		return () => window.clearTimeout(id);
	}, []);

	const [everOpened, setEverOpened] = React.useState(false);
	if (open && !everOpened) setEverOpened(true);

	const value = React.useMemo(() => ({ open, setOpen }), [open]);

	return (
		<Context.Provider value={value}>
			{children}
			{/* Mounted on first open and kept, so closing still animates out. */}
			{everOpened && (
				<React.Suspense fallback={null}>
					<CommandPaletteDialog open={open} setOpen={setOpen} />
				</React.Suspense>
			)}
		</Context.Provider>
	);
}
