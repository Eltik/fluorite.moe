/**
 * Character self-shadow: a depth map rendered from an orthographic camera
 * along the key light, fitted to the skeleton's bones. The NPR materials
 * sample it with a normal-offset bias and a 3×3 tent of hardware-compared
 * taps; a multiply-blended ground disc reuses the same map.
 */
import {
	Box3,
	CircleGeometry,
	CustomBlending,
	DoubleSide,
	type Material,
	Matrix4,
	Mesh,
	MeshDepthMaterial,
	type Object3D,
	OrthographicCamera,
	type Scene,
	ShaderMaterial,
	type SkinnedMesh,
	SrcColorFactor,
	Vector3,
	type WebGLRenderer,
	ZeroFactor,
} from "three";
import { extrasOf, nprKind } from "@/components/studio/npr/extras";
import {
	getShadowTarget,
	rigUniforms,
	SHADOW_MAP_SIZE,
	shadowUniforms,
} from "@/components/studio/npr/rig";
import {
	GROUND_SHADOW_FRAGMENT,
	GROUND_SHADOW_VERTEX,
} from "@/components/studio/npr/shaders";

/** Layer the caster pass renders. */
const CASTER_LAYER = 30;
const MARGIN = 0.3;
const RADIUS_STEP = 0.05;
const GROUND_LIFT = 0.032;
/** Ground receiver: full strength inside this radius, fading over 2.5 m. */
const GROUND_INNER = 0.3;
const GROUND_FADE = 2.5;

const bias = new Matrix4().set(
	0.5,
	0,
	0,
	0.5,
	0,
	0.5,
	0,
	0.5,
	0,
	0,
	0.5,
	0.5,
	0,
	0,
	0,
	1,
);

const box = new Box3();
const point = new Vector3();
const centre = new Vector3();
const eye = new Vector3();

export class SelfShadow {
	private readonly camera = new OrthographicCamera(-1, 1, 1, -1, 0, 10);
	private readonly caster = new MeshDepthMaterial({ side: DoubleSide });
	private readonly bones: Object3D[] = [];
	private readonly casters: Mesh[] = [];
	readonly ground: Mesh;
	private readonly groundMaterial: ShaderMaterial;

	constructor(root: Object3D) {
		const seen = new Set<Object3D>();
		root.traverse((node) => {
			const mesh = node as SkinnedMesh;
			if (!mesh.isMesh || mesh.userData.studioOutline) return;
			const mats = Array.isArray(mesh.material)
				? mesh.material
				: [mesh.material];
			const casts = mats.some(
				(m: Material) =>
					nprKind(extrasOf(m)?.shader) !== null && !m.transparent,
			);
			if (!casts) return;
			mesh.layers.enable(CASTER_LAYER);
			this.casters.push(mesh);
			for (const b of mesh.skeleton?.bones ?? []) {
				if (!seen.has(b)) {
					seen.add(b);
					this.bones.push(b);
				}
			}
		});
		this.camera.layers.set(CASTER_LAYER);
		this.groundMaterial = new ShaderMaterial({
			name: "ground shadow",
			vertexShader: GROUND_SHADOW_VERTEX,
			fragmentShader: GROUND_SHADOW_FRAGMENT,
			uniforms: {
				uShadowMap: shadowUniforms.uShadowMap,
				uShadowMatrix: shadowUniforms.uShadowMatrix,
				uShadowBias: shadowUniforms.uShadowBias,
				uShadowTexel: shadowUniforms.uShadowTexel,
				uShadowOn: shadowUniforms.uShadowOn,
				uLightDir: rigUniforms.uLightDir,
				uCentre: { value: new Vector3() },
				uInner: { value: GROUND_INNER },
				uFade: { value: GROUND_FADE },
			},
			transparent: true,
			depthWrite: false,
			blending: CustomBlending,
			blendSrc: ZeroFactor,
			blendDst: SrcColorFactor,
		});
		this.ground = new Mesh(
			new CircleGeometry(GROUND_INNER + GROUND_FADE, 64).rotateX(-Math.PI / 2),
			this.groundMaterial,
		);
		this.ground.renderOrder = -6;
		this.ground.frustumCulled = false;
		this.ground.raycast = () => {};
	}

	/** Fits the camera to the bones and renders the depth map. */
	render(gl: WebGLRenderer, scene: Scene, groundY: number): void {
		if (!this.bones.length) {
			shadowUniforms.uShadowOn.value = 0;
			return;
		}
		box.makeEmpty();
		for (const b of this.bones) {
			box.expandByPoint(point.setFromMatrixPosition(b.matrixWorld));
		}
		box.getCenter(centre);
		const raw = box.getSize(point).length() / 2 + MARGIN;
		const radius = Math.ceil(raw / RADIUS_STEP) * RADIUS_STEP;
		const L = rigUniforms.uLightDir.value;
		const cam = this.camera;
		eye.copy(centre).addScaledVector(L, radius * 2);
		cam.position.copy(eye);
		cam.up.set(0, 1, 0);
		cam.lookAt(centre);
		cam.left = -radius;
		cam.right = radius;
		cam.top = radius;
		cam.bottom = -radius;
		cam.near = 0;
		cam.far = radius * 4;
		cam.updateProjectionMatrix();
		cam.updateMatrixWorld(true);

		const texel = (2 * radius) / SHADOW_MAP_SIZE;
		shadowUniforms.uShadowBias.value.set(texel, 4 * texel);
		shadowUniforms.uShadowMatrix.value
			.copy(bias)
			.multiply(cam.projectionMatrix)
			.multiply(cam.matrixWorldInverse);
		shadowUniforms.uShadowOn.value = 1;

		const target = getShadowTarget();
		const previousTarget = gl.getRenderTarget();
		const previousOverride = scene.overrideMaterial;
		scene.overrideMaterial = this.caster;
		gl.setRenderTarget(target);
		gl.clear(false, true, false);
		gl.render(scene, cam);
		scene.overrideMaterial = previousOverride;
		gl.setRenderTarget(previousTarget);

		this.ground.position.set(centre.x, groundY + GROUND_LIFT, centre.z);
		(this.groundMaterial.uniforms.uCentre.value as Vector3).copy(
			this.ground.position,
		);
	}

	dispose(): void {
		for (const m of this.casters) m.layers.disable(CASTER_LAYER);
		this.caster.dispose();
		this.groundMaterial.dispose();
		this.ground.geometry.dispose();
		shadowUniforms.uShadowOn.value = 0;
	}
}
