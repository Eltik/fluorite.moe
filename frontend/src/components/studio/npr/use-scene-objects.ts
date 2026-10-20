/**
 * Mounts imperatively built objects into the r3f scene. Used instead of
 * `<primitive>` for objects that re-render often: the dev-time source
 * annotations added to JSX are not settable on an Object3D.
 */
import { useThree } from "@react-three/fiber";
import * as React from "react";
import type { Object3D } from "three";

export function useSceneObjects(objects: readonly Object3D[]): void {
	const scene = useThree((s) => s.scene);
	const invalidate = useThree((s) => s.invalidate);
	React.useEffect(() => {
		for (const o of objects) scene.add(o);
		invalidate();
		return () => {
			for (const o of objects) scene.remove(o);
			invalidate();
		};
	}, [scene, objects, invalidate]);
}
