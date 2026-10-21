import { describe, expect, test } from "bun:test";
import {
	AnimationClip,
	Group,
	Object3D,
	QuaternionKeyframeTrack,
	VectorKeyframeTrack,
} from "three";
import { AnimationController } from "@/components/studio/animation";
import type { ClipLibrary } from "@/components/studio/clip-library";
import type { ClipEntry } from "@/lib/clips";
import { parseMenu } from "@/lib/menu";

/** A model with a generic root bone under the Animator node. */
function model(): { root: Group; bone: Object3D; hips: Object3D } {
	const root = new Group();
	const animator = new Object3D();
	animator.name = "chr_postmodel";
	const bone = new Object3D();
	bone.name = "Root";
	const hips = new Object3D();
	hips.name = "Bip001";
	hips.position.set(0, 0.95, 0);
	root.add(animator);
	animator.add(bone);
	bone.add(hips);
	return { root, bone, hips };
}

function clip(name: string, duration: number, root?: number[]): AnimationClip {
	const tracks = [
		new QuaternionKeyframeTrack(
			"Bip001.quaternion",
			[0, duration],
			[0, 0, 0, 1, 0, 0, 0, 1],
		),
	];
	if (root) {
		const times = root.length > 3 ? [0, duration] : [0];
		tracks.push(new VectorKeyframeTrack("Root.position", times, root));
	}
	return new AnimationClip(name, duration, tracks);
}

function library(
	entries: ClipEntry[],
	clips: Record<string, AnimationClip>,
): ClipLibrary {
	return {
		entries,
		load: (e) => Promise.resolve(clips[e.file]),
		prefetch: () => {},
		isReady: () => true,
	};
}

function entry(file: string, extra: Partial<ClipEntry> = {}): ClipEntry {
	return {
		file,
		name: file,
		clip: file,
		source: "ui",
		duration: 2,
		loop: true,
		...extra,
	};
}

const TEAM = "a_x_uiteam_idle_01.glb";
const WALK = "a_x_battle_walk.glb";
const START = "a_x_ui_overview_start_01.glb";
const LOOP = "a_x_ui_overview_loop_01.glb";

function setup(rootMotion = false) {
	const m = model();
	const entries = [
		entry(TEAM, { rootNode: "Root", rootStart: [-0.9333, 0, -1.1749] }),
		entry(WALK, {
			loop: false,
			rootMotion: true,
			rootNode: "Root",
			rootStart: [0, 0, 0.5],
			rootEnd: [0, 0, 1.5],
		}),
		entry(START, { loop: false }),
		entry(LOOP),
	];
	const clips = {
		[TEAM]: clip("team", 2, [-0.9333, 0, -1.1749]),
		[WALK]: clip("walk", 2, [0, 0, 0.5, 0, 0, 1.5]),
		[START]: clip("start", 1),
		[LOOP]: clip("loop", 2),
	};
	const menu = parseMenu({
		default: "Overview.Idle",
		states: {
			"Overview.Start": {
				clip: START,
				length: 1,
				exit: { to: "Overview.Idle", at: 0.9, fade: 0.1 },
			},
			"Overview.Idle": { clip: LOOP, length: 2, loop: true },
		},
		moments: [
			{ id: "overview", state: "Overview.Start", idle: "Overview.Idle" },
		],
	});
	const controller = new AnimationController(m.root, library(entries, clips), {
		rootMotion,
		menu,
	});
	return { ...m, controller };
}

describe("AnimationController", () => {
	test("play() with nothing loaded starts the opening moment once", async () => {
		const { controller } = setup();
		// A transport play before anything is selected must not re-enter
		// itself (the selection path once called the public `play`).
		controller.play();
		await Promise.resolve();
		await Promise.resolve();
		expect(controller.state.moment).toBe("overview");
		expect(controller.state.menuState).toBe("Overview.Start");
		controller.update(0.95);
		await Promise.resolve();
		await Promise.resolve();
		expect(controller.state.menuState).toBe("Overview.Idle");
		controller.dispose();
	});

	test("a deep-linked clip loads and plays", async () => {
		const { controller } = setup();
		expect(await controller.select("a_x_uiteam_idle_01")).toBe(true);
		expect(controller.state.clip).toBe("a_x_uiteam_idle_01");
		expect(controller.state.playing).toBe(true);
		controller.restart();
		expect(controller.time).toBe(0);
		controller.dispose();
	});

	test("in place keeps a parked root at rest", async () => {
		const { controller, bone } = setup(false);
		await controller.select("a_x_uiteam_idle_01");
		controller.update(0.5);
		expect(bone.position.toArray()).toEqual([0, 0, 0]);
		await controller.select("a_x_battle_walk");
		controller.update(1);
		expect(bone.position.x).toBeCloseTo(0);
		expect(bone.position.z).toBeCloseTo(0);
		controller.dispose();
	});

	test("root motion travels from rest by the clip's delta", async () => {
		const { controller, bone } = setup(true);
		await controller.select("a_x_uiteam_idle_01");
		controller.update(0.5);
		expect(bone.position.toArray()).toEqual([0, 0, 0]);
		await controller.select("a_x_battle_walk", false);
		controller.seek(1);
		expect(bone.position.z).toBeCloseTo(0.5);
		controller.setRootMotion(false);
		expect(bone.position.z).toBeCloseTo(0);
		controller.dispose();
	});
});
