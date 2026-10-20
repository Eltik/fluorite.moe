import { toastManager } from "@/components/ui/toast";

export async function copyText(text: string, label: string): Promise<void> {
	try {
		await navigator.clipboard.writeText(text);
		toastManager.add({
			title: `${label} copied`,
			type: "success",
			timeout: 1800,
		});
	} catch {
		toastManager.add({ title: "Clipboard unavailable", type: "error" });
	}
}
