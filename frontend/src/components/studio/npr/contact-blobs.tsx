/**
 * Soft ground contact blobs under the pelvis and both feet of a rigged
 * character. Each follows its bones' xz position and fades as the bones lift
 * above their rest height.
 */
import { useFrame } from "@react-three/fiber";
import * as React from "react";
import {
	type Bone,
	CanvasTexture,
	Mesh,
	MeshBasicMaterial,
	type Object3D,
	PlaneGeometry,
	Vector3,
} from "three";
import { useSceneObjects } from "@/components/studio/npr/use-scene-objects";

interface BlobDef {
	bones: string[];
	size: number;
	opacity: number;
	/** Height above rest at which the blob has faded out. */
	fadeHeight: number;
}

const BLOBS: BlobDef[] = [
	{ bones: ["Bip001_Pelvis"], size: 0.8, opacity: 0.22, fadeHeight: 1 },
	{
		bones: ["Bip001_L_Foot", "Bip001_L_Toe0"],
		size: 0.3,
		opacity: 0.3,
		fadeHeight: 0.4,
	},
	{
		bones: ["Bip001_R_Foot", "Bip001_R_Toe0"],
		size: 0.3,
		opacity: 0.3,
		fadeHeight: 0.4,
	},
];
const BLOB_Y = 0.001;
const RENDER_ORDER = -5;

let gradient: CanvasTexture | null = null;

/** 64 px radial alpha: 1, .85, .5, .15, 0 at 0, 25, 50, 75, 100 %. */
function getGradient(): CanvasTexture {
	if (gradient) return gradient;
	const canvas = document.createElement("canvas");
	canvas.width = canvas.height = 64;
	const ctx = canvas.getContext("2d");
	if (ctx) {
		const g = ctx.createRadialGradient(32, 32, 0, 32, 32, 32);
		g.addColorStop(0, "rgba(255,255,255,1)");
		g.addColorStop(0.25, "rgba(255,255,255,0.85)");
		g.addColorStop(0.5, "rgba(255,255,255,0.5)");
		g.addColorStop(0.75, "rgba(255,255,255,0.15)");
		g.addColorStop(1, "rgba(255,255,255,0)");
		ctx.fillStyle = g;
		ctx.fillRect(0, 0, 64, 64);
	}
	gradient = new CanvasTexture(canvas);
	return gradient;
}

interface Blob {
	def: BlobDef;
	bones: Object3D[];
	mesh: Mesh;
	restY: number;
}

function findBones(root: Object3D, names: string[]): Object3D[] {
	const found: Object3D[] = [];
	root.traverse((node) => {
		if ((node as Bone).isBone && names.includes(node.name)) found.push(node);
	});
	return found;
}

const centre = new Vector3();
const point = new Vector3();

function boneCentre(bones: Object3D[], out: Vector3): Vector3 {
	out.set(0, 0, 0);
	for (const b of bones) out.add(point.setFromMatrixPosition(b.matrixWorld));
	return out.divideScalar(bones.length);
}

export function ContactBlobs({
	root,
	/** Changes whenever the model is re-placed; resets the rest heights. */
	placementKey,
}: {
	root: Object3D;
	placementKey: string | number;
}): null {
	const blobs = React.useMemo(() => {
		const geometry = new PlaneGeometry(1, 1).rotateX(-Math.PI / 2);
		const list: Blob[] = [];
		for (const def of BLOBS) {
			const bones = findBones(root, def.bones);
			if (!bones.length) continue;
			const mesh = new Mesh(
				geometry,
				new MeshBasicMaterial({
					color: 0x000000,
					alphaMap: getGradient(),
					transparent: true,
					depthWrite: false,
					toneMapped: false,
					opacity: def.opacity,
				}),
			);
			mesh.renderOrder = RENDER_ORDER;
			mesh.raycast = () => {};
			list.push({ def, bones, mesh, restY: Number.NaN });
		}
		return { geometry, list };
	}, [root]);

	React.useEffect(
		() => () => {
			blobs.geometry.dispose();
			for (const b of blobs.list)
				(b.mesh.material as MeshBasicMaterial).dispose();
		},
		[blobs],
	);

	// Rest heights are measured on the first frame after (re)placement.
	// biome-ignore lint/correctness/useExhaustiveDependencies: placementKey is the trigger
	React.useEffect(() => {
		for (const b of blobs.list) b.restY = Number.NaN;
	}, [blobs, placementKey]);

	useFrame(() => {
		for (const b of blobs.list) {
			boneCentre(b.bones, centre);
			if (Number.isNaN(b.restY)) b.restY = centre.y;
			const h = Math.max(centre.y - b.restY, 0);
			let l = Math.min(h / b.def.fadeHeight, 1);
			l = l * l * (3 - 2 * l);
			b.mesh.position.set(centre.x, BLOB_Y, centre.z);
			b.mesh.scale.setScalar(b.def.size * (1 + 0.5 * l));
			(b.mesh.material as MeshBasicMaterial).opacity = b.def.opacity * (1 - l);
		}
	});

	const objects = React.useMemo(() => blobs.list.map((b) => b.mesh), [blobs]);
	useSceneObjects(objects);
	return null;
}
