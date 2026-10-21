import * as React from "react";
import {
	AlertDialog,
	AlertDialogClose,
	AlertDialogDescription,
	AlertDialogFooter,
	AlertDialogHeader,
	AlertDialogPopup,
	AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

const SEEN_KEY = "fluorite.vibecoded-notice";


function hasSeenNotice(): boolean {
	try {
		return window.localStorage.getItem(SEEN_KEY) === "1";
	} catch {
		return false;
	}
}

function markNoticeSeen(): void {
	try {
		window.localStorage.setItem(SEEN_KEY, "1");
	} catch {
		// Storage unavailable: the notice simply shows again next visit.
	}
}

/** One-time warning shown on a visitor's first load. */
export function VibecodedNotice(): React.ReactElement {
	const [open, setOpen] = React.useState(false);

	React.useEffect(() => {
		if (!hasSeenNotice()) setOpen(true);
	}, []);

	return (
		<AlertDialog
			onOpenChange={(next) => {
				setOpen(next);
				if (!next) markNoticeSeen();
			}}
			open={open}
		>
			<AlertDialogPopup>
				<AlertDialogHeader>
					<AlertDialogTitle>Warning: Vibecoded</AlertDialogTitle>
					<AlertDialogDescription>Due to popular demand, I released this version of the website completely vibecoded. I don't support vibecoded production websites, so just know that this will be entirely handwritten once I got time. Just want to be transparent with people so they know.</AlertDialogDescription>
				</AlertDialogHeader>
				<AlertDialogFooter>
					<AlertDialogClose render={<Button />}>Got it</AlertDialogClose>
				</AlertDialogFooter>
			</AlertDialogPopup>
		</AlertDialog>
	);
}
