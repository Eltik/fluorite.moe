import { useQuery } from "@tanstack/react-query";
import { RotateCcwIcon } from "lucide-react";
import type * as React from "react";
import { RelativeTime } from "@/components/shell/relative-time";
import { Button } from "@/components/ui/button";
import {
	Popover,
	PopoverPopup,
	PopoverTitle,
	PopoverTrigger,
} from "@/components/ui/popover";
import { API_LABEL } from "@/lib/api";
import { statusQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";

type Health = "loading" | "online" | "offline";

const DOT: Record<Health, string> = {
	loading: "bg-muted-foreground/40 animate-pulse",
	online: "bg-success",
	offline: "bg-destructive",
};

const LABEL: Record<Health, string> = {
	loading: "Connecting",
	online: "Online",
	offline: "Offline",
};

export function StatusIndicator(): React.ReactElement {
	const status = useQuery(statusQuery());
	const health: Health = status.isPending
		? "loading"
		: status.isError
			? "offline"
			: "online";

	return (
		<Popover>
			<PopoverTrigger
				render={
					<Button
						aria-label={`API status: ${LABEL[health]}`}
						className="gap-2 px-2 font-normal text-muted-foreground"
						size="sm"
						variant="ghost"
					/>
				}
			>
				<span className="relative flex size-2">
					{health === "online" && (
						<span className="absolute inline-flex size-full animate-ping rounded-full bg-success opacity-40 motion-reduce:hidden [animation-iteration-count:2]" />
					)}
					<span
						className={cn(
							"relative inline-flex size-2 rounded-full",
							DOT[health],
						)}
					/>
				</span>
				<span className="hidden text-xs md:inline">
					{status.data ? (
						<>
							API{" "}
							<span className="font-mono tabular-nums">
								v{status.data.version}
							</span>
						</>
					) : (
						`API ${LABEL[health].toLowerCase()}`
					)}
				</span>
			</PopoverTrigger>
			<PopoverPopup align="end" className="w-72">
				<div className="flex items-center justify-between gap-2">
					<PopoverTitle className="text-sm">Data API</PopoverTitle>
					<span
						className={cn(
							"inline-flex items-center gap-1.5 text-xs",
							health === "offline"
								? "text-destructive-foreground"
								: "text-muted-foreground",
						)}
					>
						<span className={cn("size-1.5 rounded-full", DOT[health])} />
						{LABEL[health]}
					</span>
				</div>
				{status.data ? (
					<dl className="mt-3 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
						<dt className="text-muted-foreground">Game version</dt>
						<dd className="text-end font-mono">{status.data.version}</dd>
						<dt className="text-muted-foreground">Resources</dt>
						<dd
							className="truncate text-end font-mono"
							title={status.data.resVersion}
						>
							{status.data.resVersion}
						</dd>
						<dt className="text-muted-foreground">Schema</dt>
						<dd className="text-end font-mono">v{status.data.schemaVersion}</dd>
						<dt className="text-muted-foreground">Updated</dt>
						<dd className="text-end">
							<RelativeTime iso={status.data.updatedAt} />
						</dd>
					</dl>
				) : (
					<p className="mt-2 text-muted-foreground text-xs leading-relaxed">
						{health === "offline"
							? "The data API is not reachable. Browsing will resume once it is back."
							: "Checking the data API…"}
					</p>
				)}
				<div className="mt-3 flex items-center justify-between gap-2 border-t pt-3">
					<code className="truncate text-muted-foreground text-xs">
						{API_LABEL}
					</code>
					<Button
						aria-label="Recheck status"
						loading={status.isFetching}
						onClick={() => void status.refetch()}
						size="icon-xs"
						variant="ghost"
					>
						<RotateCcwIcon />
					</Button>
				</div>
			</PopoverPopup>
		</Popover>
	);
}
