import { createFileRoute, Outlet } from "@tanstack/react-router";

export const Route = createFileRoute("/tables")({
	staticData: { section: "Data", crumb: "Tables" },
	component: Outlet,
});
