import { SearchIcon, XIcon } from "lucide-react";
import * as React from "react";
import {
	InputGroup,
	InputGroupAddon,
	InputGroupInput,
} from "@/components/ui/input-group";
import { cn } from "@/lib/utils";

/**
 * Filter input whose committed value usually lives in the URL. Typing updates
 * local state immediately and commits after `delay` ms, so URL writes (and the
 * re-filtering they trigger) are debounced while the field stays responsive.
 * External changes (back/forward) flow back into the field.
 */
export function SearchInput({
	value,
	onCommit,
	placeholder,
	label,
	className,
	delay = 150,
	inputRef,
	onKeyDown,
}: {
	value: string;
	onCommit: (value: string) => void;
	placeholder?: string;
	label: string;
	className?: string;
	delay?: number;
	inputRef?: React.Ref<HTMLInputElement>;
	onKeyDown?: React.KeyboardEventHandler<HTMLInputElement>;
}): React.ReactElement {
	const [draft, setDraft] = React.useState(value);
	const committed = React.useRef(value);
	const commitRef = React.useRef(onCommit);
	commitRef.current = onCommit;

	React.useEffect(() => {
		if (value !== committed.current) {
			committed.current = value;
			setDraft(value);
		}
	}, [value]);

	React.useEffect(() => {
		if (draft === committed.current) return;
		const id = window.setTimeout(() => {
			committed.current = draft;
			commitRef.current(draft);
		}, delay);
		return () => window.clearTimeout(id);
	}, [draft, delay]);

	return (
		<InputGroup className={cn("w-full", className)}>
			<InputGroupInput
				aria-label={label}
				onChange={(event) => setDraft(event.target.value)}
				onKeyDown={(event) => {
					if (event.key === "Escape" && draft) {
						event.preventDefault();
						event.stopPropagation();
						setDraft("");
					}
					onKeyDown?.(event);
				}}
				placeholder={placeholder}
				ref={inputRef}
				size="sm"
				type="search"
				value={draft}
			/>
			<InputGroupAddon>
				<SearchIcon />
			</InputGroupAddon>
			{draft && (
				<InputGroupAddon align="inline-end">
					<button
						aria-label="Clear filter"
						className="-me-1 inline-flex size-5 cursor-pointer items-center justify-center rounded-sm text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
						onClick={() => setDraft("")}
						type="button"
					>
						<XIcon className="size-3.5" />
					</button>
				</InputGroupAddon>
			)}
		</InputGroup>
	);
}
