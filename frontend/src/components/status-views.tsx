import { useQueryErrorResetBoundary } from "@tanstack/react-query";
import {
	type ErrorComponentProps,
	Link,
	useRouter,
} from "@tanstack/react-router";
import {
	CompassIcon,
	RotateCcwIcon,
	ServerOffIcon,
	TriangleAlertIcon,
} from "lucide-react";
import type * as React from "react";
import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { API_URL, isApiError } from "@/lib/api";
import { cn } from "@/lib/utils";

function describe(error: unknown): {
	title: string;
	message: string;
	offline: boolean;
} {
	if (isApiError(error)) {
		if (error.status === 0) {
			return {
				title: "API unreachable",
				message: `Could not connect to ${API_URL}. Start fluorite-api or set VITE_API_URL.`,
				offline: true,
			};
		}
		if (error.status === 404) {
			return { title: "Not found", message: error.message, offline: false };
		}
		return {
			title: `Request failed (${error.status})`,
			message: error.message,
			offline: false,
		};
	}
	return {
		title: "Something went wrong",
		message: error instanceof Error ? error.message : String(error),
		offline: false,
	};
}

export function ErrorState({
	error,
	onRetry,
	className,
}: {
	error: unknown;
	onRetry?: () => void;
	className?: string;
}): React.ReactElement {
	const { title, message, offline } = describe(error);
	const Icon = offline ? ServerOffIcon : TriangleAlertIcon;
	return (
		<Empty className={className} role="alert">
			<EmptyHeader>
				<EmptyMedia variant="icon">
					<Icon />
				</EmptyMedia>
				<EmptyTitle>{title}</EmptyTitle>
				<EmptyDescription className="break-words">{message}</EmptyDescription>
			</EmptyHeader>
			{onRetry && (
				<EmptyContent>
					<Button onClick={onRetry} variant="outline">
						<RotateCcwIcon />
						Try again
					</Button>
				</EmptyContent>
			)}
		</Empty>
	);
}

export function DefaultError({
	error,
	reset,
}: ErrorComponentProps): React.ReactElement {
	const router = useRouter();
	const queryBoundary = useQueryErrorResetBoundary();

	useEffect(() => {
		queryBoundary.clearReset();
	}, [queryBoundary]);

	return (
		<ErrorState
			className="min-h-[60vh]"
			error={error}
			onRetry={() => {
				queryBoundary.reset();
				reset();
				void router.invalidate();
			}}
		/>
	);
}

export function DefaultNotFound(): React.ReactElement {
	return (
		<Empty className="min-h-[60vh]">
			<EmptyHeader>
				<EmptyMedia variant="icon">
					<CompassIcon />
				</EmptyMedia>
				<EmptyTitle>Page not found</EmptyTitle>
				<EmptyDescription>
					This page does not exist, or the data behind it has moved.
				</EmptyDescription>
			</EmptyHeader>
			<EmptyContent>
				<Button render={<Link to="/" />} variant="outline">
					Back to overview
				</Button>
			</EmptyContent>
		</Empty>
	);
}

export function DefaultPending(): React.ReactElement {
	return (
		<output
			aria-busy="true"
			aria-label="Loading"
			className="flex flex-col gap-4 p-4 md:p-6"
		>
			<Skeleton className="h-7 w-48" />
			<Skeleton className="h-4 w-80 max-w-full" />
			<div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
				<Skeleton className="h-28" />
				<Skeleton className="h-28" />
				<Skeleton className="h-28" />
			</div>
		</output>
	);
}

export function ListSkeleton({
	rows = 12,
	className,
}: {
	rows?: number;
	className?: string;
}): React.ReactElement {
	return (
		<div
			aria-busy="true"
			className={cn("flex flex-col gap-1.5 p-2", className)}
		>
			{Array.from({ length: rows }, (_, i) => (
				<Skeleton
					className="h-6"
					// biome-ignore lint/suspicious/noArrayIndexKey: static placeholder rows
					key={i}
					style={{ width: `${55 + ((i * 37) % 40)}%` }}
				/>
			))}
		</div>
	);
}
