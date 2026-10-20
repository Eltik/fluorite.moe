import { createFileRoute, Outlet } from "@tanstack/react-router";

export const Route = createFileRoute("/story")({
	staticData: { section: "Catalog", crumb: "Story" },
	component: Outlet,
});
