import { QueryClient } from "@tanstack/react-query";
import { createRouter as createTanStackRouter } from "@tanstack/react-router";
import { setupRouterSsrQueryIntegration } from "@tanstack/react-router-ssr-query";
import {
	DefaultError,
	DefaultNotFound,
	DefaultPending,
} from "@/components/status-views";
import { isApiError } from "@/lib/api";
import { routeTree } from "./routeTree.gen";

export function getRouter() {
	const queryClient = new QueryClient({
		defaultOptions: {
			queries: {
				staleTime: 30_000,
				refetchOnWindowFocus: false,
				retry: (count, error) => {
					// 4xx/5xx answers are deterministic; only retry transport failures.
					if (isApiError(error) && error.status !== 0) return false;
					return count < 2;
				},
			},
		},
	});

	const router = createTanStackRouter({
		routeTree,
		context: { queryClient },
		scrollRestoration: true,
		defaultPreload: "intent",
		defaultPreloadStaleTime: 0,
		defaultPendingMs: 150,
		defaultPendingMinMs: 200,
		defaultErrorComponent: DefaultError,
		defaultNotFoundComponent: DefaultNotFound,
		defaultPendingComponent: DefaultPending,
	});

	setupRouterSsrQueryIntegration({ router, queryClient });

	return router;
}

declare module "@tanstack/react-router" {
	interface Register {
		router: ReturnType<typeof getRouter>;
	}
}
