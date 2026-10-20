import { TanStackDevtools } from "@tanstack/react-devtools";
import { TanStackRouterDevtoolsPanel } from "@tanstack/react-router-devtools";
import type * as React from "react";

/** Loaded lazily and only when `import.meta.env.DEV` is true. */
export default function Devtools(): React.ReactElement {
	return (
		<TanStackDevtools
			config={{ position: "bottom-left" }}
			plugins={[
				{ name: "TanStack Router", render: <TanStackRouterDevtoolsPanel /> },
			]}
		/>
	);
}
