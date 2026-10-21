/**
 * Debug helpers for the Studio's Display and Inspect tools: overlay copies
 * of a mesh (wireframe, selection tint), a pose-aware vertex normals helper
 * and the UV checker texture. Everything here is built on demand and
 * disposed by its owner; nothing is rebuilt per frame.
 */
import {
	BufferAttribute,
	BufferGeometry,
	CanvasTexture,
	Color,
	LineBasicMaterial,
	LineSegments,
	type Material,
	Mesh,
	MeshBasicMaterial,
	type Object3D,
	RepeatWrapping,
	SkinnedMesh,
	SRGBColorSpace,
	Vector3,
} from "three";

/**
 * Marks an overlay so the shading, shadow and picking passes skip it. The
 * NPR self-shadow and `meshesOf` already honour `studioOutline`; helpers
 * reuse that flag rather than teaching every pass a second one.
 */
export function markHelper(object: Object3D): void {
	object.userData.studioOutline = true;
	object.userData.studioHelper = true;
	object.raycast = () => {};
}

export function isHelper(object: Object3D): boolean {
	return Boolean(object.userData.studioOutline || object.userData.studioHelper);
}

/**
 * A mesh that draws `mesh`'s geometry with another material, following its
 * transform (and skeleton) as a child with an identity local matrix.
 */
export function makeOverlay(mesh: Mesh, material: Material | Material[]): Mesh {
	let overlay: Mesh;
	if ((mesh as SkinnedMesh).isSkinnedMesh) {
		const skinned = mesh as SkinnedMesh;
		const o = new SkinnedMesh(mesh.geometry, material);
		o.bindMode = skinned.bindMode;
		o.bind(skinned.skeleton, skinned.bindMatrix);
		overlay = o;
	} else {
		overlay = new Mesh(mesh.geometry, material);
	}
	overlay.frustumCulled = mesh.frustumCulled;
	overlay.matrixAutoUpdate = false;
	markHelper(overlay);
	return overlay;
}

/** Shared by every wireframe overlay; the stage recolours it per backdrop. */
export const wireOverlayColor = new Color("#d8d2ff");

export function wireOverlayMaterial(): MeshBasicMaterial {
	const m = new MeshBasicMaterial({
		name: "wire overlay",
		wireframe: true,
		transparent: true,
		opacity: 0.55,
		depthWrite: false,
		polygonOffset: true,
		polygonOffsetFactor: -1,
		polygonOffsetUnits: -1,
		toneMapped: false,
	});
	m.color = wireOverlayColor;
	return m;
}

/** Selection tint plus a brighter wire, both drawn over the picked mesh. */
export function selectionMaterials(): [MeshBasicMaterial, MeshBasicMaterial] {
	const tint = new MeshBasicMaterial({
		name: "selection",
		color: new Color("#8b7cf6"),
		transparent: true,
		opacity: 0.28,
		depthWrite: false,
		polygonOffset: true,
		polygonOffsetFactor: -2,
		polygonOffsetUnits: -2,
		toneMapped: false,
	});
	const wire = new MeshBasicMaterial({
		name: "selection wire",
		color: new Color("#c4b5fd"),
		wireframe: true,
		transparent: true,
		opacity: 0.45,
		depthWrite: false,
		polygonOffset: true,
		polygonOffsetFactor: -2,
		polygonOffsetUnits: -2,
		toneMapped: false,
	});
	return [tint, wire];
}

// ------------------------------------------------------------- normals

/** Total normals drawn at once, across every mesh in the helper. */
export const NORMALS_CAP = 16000;

const _p = new Vector3();
const _n = new Vector3();
const _q = new Vector3();

interface NormalsSource {
	mesh: Mesh;
	/** Every `stride`-th vertex, so the cap spreads across the surface. */
	stride: number;
	count: number;
}

/**
 * Vertex normals as line segments, in world space, for the current pose:
 * skinned meshes go through their bone transforms (the stock
 * VertexNormalsHelper shows the bind pose). `update()` refreshes the
 * positions in place; it only runs while an animation is playing.
 */
export class NormalsHelper {
	readonly object: LineSegments;
	private readonly sources: NormalsSource[] = [];
	private readonly positions: Float32Array;
	private readonly size: number;

	constructor(meshes: Mesh[], size: number, cap = NORMALS_CAP) {
		const usable = meshes.filter(
			(m) =>
				m.visible &&
				m.geometry.attributes.position &&
				m.geometry.attributes.normal,
		);
		const total = usable.reduce(
			(s, m) => s + m.geometry.attributes.position.count,
			0,
		);
		const stride = Math.max(1, Math.ceil(total / Math.max(cap, 1)));
		let lines = 0;
		for (const mesh of usable) {
			const count = Math.ceil(mesh.geometry.attributes.position.count / stride);
			this.sources.push({ mesh, stride, count });
			lines += count;
		}
		this.size = size;
		this.positions = new Float32Array(lines * 6);
		const geometry = new BufferGeometry();
		geometry.setAttribute("position", new BufferAttribute(this.positions, 3));
		const material = new LineBasicMaterial({
			color: new Color("#4cd7ff"),
			transparent: true,
			opacity: 0.9,
			depthTest: true,
			depthWrite: false,
			toneMapped: false,
		});
		this.object = new LineSegments(geometry, material);
		this.object.name = "vertex normals";
		this.object.frustumCulled = false;
		this.object.renderOrder = 10;
		markHelper(this.object);
		this.update();
	}

	get count(): number {
		return this.positions.length / 6;
	}

	/** Recomputes every segment for the meshes' current world pose. */
	update(): void {
		const out = this.positions;
		let o = 0;
		for (const { mesh, stride, count } of this.sources) {
			mesh.updateMatrixWorld();
			const pos = mesh.geometry.attributes.position;
			const nor = mesh.geometry.attributes.normal;
			const skinned = (mesh as SkinnedMesh).isSkinnedMesh
				? (mesh as SkinnedMesh)
				: null;
			for (let k = 0; k < count; k++) {
				const i = Math.min(k * stride, pos.count - 1);
				_p.fromBufferAttribute(pos, i);
				_n.fromBufferAttribute(nor, i).normalize().multiplyScalar(this.size);
				_q.copy(_p).add(_n);
				if (skinned) {
					// Skinning a second point along the normal yields the posed
					// direction without a separate normal-matrix path.
					skinned.applyBoneTransform(i, _p);
					skinned.applyBoneTransform(i, _q);
				}
				_p.applyMatrix4(mesh.matrixWorld);
				_q.applyMatrix4(mesh.matrixWorld);
				out[o++] = _p.x;
				out[o++] = _p.y;
				out[o++] = _p.z;
				out[o++] = _q.x;
				out[o++] = _q.y;
				out[o++] = _q.z;
			}
		}
		(this.object.geometry.attributes.position as BufferAttribute).needsUpdate =
			true;
	}

	/** Frees the GPU buffers; the owner (an r3f primitive) detaches the object. */
	dispose(): void {
		this.object.geometry.dispose();
		(this.object.material as Material).dispose();
	}
}

// ------------------------------------------------------------- UV checker

let checker: CanvasTexture | null = null;

/** An 8×8 checker with a hue gradient and a labelled grid, for UV inspection. */
export function getUvChecker(): CanvasTexture {
	if (checker) return checker;
	const size = 1024;
	const cells = 8;
	const cell = size / cells;
	const canvas = document.createElement("canvas");
	canvas.width = canvas.height = size;
	const ctx = canvas.getContext("2d");
	if (ctx) {
		for (let y = 0; y < cells; y++) {
			for (let x = 0; x < cells; x++) {
				const hue = ((x + y * 0.5) / cells) * 300;
				const dark = (x + y) % 2 === 0;
				ctx.fillStyle = `hsl(${hue} ${dark ? 45 : 60}% ${dark ? 38 : 72}%)`;
				ctx.fillRect(x * cell, y * cell, cell, cell);
			}
		}
		ctx.strokeStyle = "rgba(255,255,255,0.55)";
		ctx.lineWidth = 2;
		for (let i = 0; i <= cells; i++) {
			ctx.beginPath();
			ctx.moveTo(i * cell, 0);
			ctx.lineTo(i * cell, size);
			ctx.moveTo(0, i * cell);
			ctx.lineTo(size, i * cell);
			ctx.stroke();
		}
		ctx.fillStyle = "rgba(0,0,0,0.65)";
		ctx.font = `${Math.round(cell * 0.28)}px ui-monospace, monospace`;
		ctx.textBaseline = "top";
		for (let y = 0; y < cells; y++) {
			for (let x = 0; x < cells; x++) {
				ctx.fillText(
					`${String.fromCharCode(65 + x)}${y + 1}`,
					x * cell + cell * 0.08,
					y * cell + cell * 0.08,
				);
			}
		}
	}
	checker = new CanvasTexture(canvas);
	checker.colorSpace = SRGBColorSpace;
	checker.wrapS = checker.wrapT = RepeatWrapping;
	checker.anisotropy = 8;
	// glTF UVs have V pointing down.
	checker.flipY = false;
	return checker;
}
