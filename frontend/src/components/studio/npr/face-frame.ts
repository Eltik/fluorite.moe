/**
 * Head axes for the SDF face shadow. The face mesh's bind-pose UV layout
 * gives a forward (middle-band normals) and right (left-to-right centroid)
 * axis, expressed in the head bone's space so they follow the animation.
 */
import {
	type BufferAttribute,
	type InterleavedBufferAttribute,
	type IUniform,
	Matrix4,
	type Object3D,
	type SkinnedMesh,
	Vector3,
} from "three";

export interface FaceFrame {
	/** The head bone (first ancestor of the dominant bone named `head`). */
	head: Object3D;
	/** Head-local axes. */
	forward: Vector3;
	right: Vector3;
}

export interface FaceUniforms {
	uHeadForward: IUniform<Vector3>;
	uHeadRight: IUniform<Vector3>;
	uHeadPos: IUniform<Vector3>;
}

const HEAD = /head/i;

type Attr = BufferAttribute | InterleavedBufferAttribute;

function headOf(bone: Object3D): Object3D | null {
	for (let n: Object3D | null = bone; n; n = n.parent) {
		if (HEAD.test(n.name)) return n;
	}
	return null;
}

/** Computes the frame from a skin mesh in its bind pose, or null. */
export function computeFaceFrame(mesh: SkinnedMesh): FaceFrame | null {
	const g = mesh.geometry;
	const pos = g.attributes.position as Attr | undefined;
	const uv = g.attributes.uv as Attr | undefined;
	const nrm = g.attributes.normal as Attr | undefined;
	const joints = g.attributes.skinIndex as Attr | undefined;
	const weights = g.attributes.skinWeight as Attr | undefined;
	if (!pos || !uv || !nrm || !joints || !weights) return null;

	const left = new Vector3();
	const right = new Vector3();
	const forward = new Vector3();
	let leftN = 0;
	let rightN = 0;
	const boneWeight = new Map<number, number>();
	for (let i = 0; i < pos.count; i++) {
		const u = uv.getX(i);
		const v = uv.getY(i);
		if (u < 0.2 || u > 0.8 || v < 0.25 || v > 0.7) continue;
		for (let k = 0; k < 4; k++) {
			const w = weights.getComponent(i, k);
			if (w <= 0) continue;
			const j = joints.getComponent(i, k);
			boneWeight.set(j, (boneWeight.get(j) ?? 0) + w);
		}
		if (u < 0.4) {
			left.x += pos.getX(i);
			left.y += pos.getY(i);
			left.z += pos.getZ(i);
			leftN++;
		} else if (u > 0.6) {
			right.x += pos.getX(i);
			right.y += pos.getY(i);
			right.z += pos.getZ(i);
			rightN++;
		} else {
			forward.x += nrm.getX(i);
			forward.y += nrm.getY(i);
			forward.z += nrm.getZ(i);
		}
	}
	if (!leftN || !rightN || forward.lengthSq() < 1e-8) return null;
	forward.normalize();
	left.divideScalar(leftN);
	right.divideScalar(rightN);
	const side = right.sub(left);
	side.addScaledVector(forward, -side.dot(forward));
	if (side.lengthSq() < 1e-8) return null;
	side.normalize();

	let dominant = -1;
	let best = 0;
	for (const [j, w] of boneWeight) {
		if (w > best) {
			best = w;
			dominant = j;
		}
	}
	const bone = mesh.skeleton?.bones[dominant];
	const inverse = mesh.skeleton?.boneInverses[dominant];
	if (!bone || !inverse) return null;
	const head = headOf(bone);
	if (!head) return null;

	// mesh (bind) space → bone space → head space, at the rest pose.
	bone.updateWorldMatrix(true, false);
	head.updateWorldMatrix(true, false);
	const toBone = new Matrix4().multiplyMatrices(inverse, mesh.bindMatrix);
	const toHead = new Matrix4()
		.copy(head.matrixWorld)
		.invert()
		.multiply(bone.matrixWorld)
		.multiply(toBone);
	return {
		head,
		forward: forward.transformDirection(toHead),
		right: side.transformDirection(toHead),
	};
}

export function createFaceUniforms(): FaceUniforms {
	return {
		uHeadForward: { value: new Vector3(0, 0, 1) },
		uHeadRight: { value: new Vector3(1, 0, 0) },
		uHeadPos: { value: new Vector3() },
	};
}

/** Writes the head's current world axes and position into the uniforms. */
export function updateFaceUniforms(
	frame: FaceFrame,
	uniforms: FaceUniforms,
): void {
	const m = frame.head.matrixWorld;
	uniforms.uHeadForward.value.copy(frame.forward).transformDirection(m);
	uniforms.uHeadRight.value.copy(frame.right).transformDirection(m);
	uniforms.uHeadPos.value.setFromMatrixPosition(m);
}
