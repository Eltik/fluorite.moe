/**
 * Contact shadows vendored from drei's ContactShadows. drei never disposes
 * its render targets, plane geometry or materials, so every remount leaked
 * GPU memory. This copy disposes them whenever they are rebuilt or the
 * component unmounts, and re-bakes on a `bakeKey` change instead of needing
 * a remount.
 */
import { useFrame, useThree } from "@react-three/fiber";
import * as React from "react";
import {
	Color,
	type Group,
	type IUniform,
	Mesh,
	MeshDepthMaterial,
	type OrthographicCamera,
	PlaneGeometry,
	ShaderMaterial,
	UniformsUtils,
	WebGLRenderTarget,
} from "three";
import { HorizontalBlurShader } from "three/examples/jsm/shaders/HorizontalBlurShader.js";
import { VerticalBlurShader } from "three/examples/jsm/shaders/VerticalBlurShader.js";

interface ContactShadowsProps {
	/** Any change re-runs the bake (`frames` more renders). */
	bakeKey?: string;
	blur?: number;
	color?: string;
	far?: number;
	frames?: number;
	opacity?: number;
	position?: [number, number, number];
	resolution?: number;
	scale?: number;
}

function blurMaterial(shader: {
	uniforms: Record<string, IUniform>;
	vertexShader: string;
	fragmentShader: string;
}): ShaderMaterial {
	const material = new ShaderMaterial({
		fragmentShader: shader.fragmentShader,
		uniforms: UniformsUtils.clone(shader.uniforms),
		vertexShader: shader.vertexShader,
	});
	material.depthTest = false;
	return material;
}

export function ContactShadows({
	bakeKey,
	blur = 1,
	color = "#000000",
	far = 10,
	frames = Number.POSITIVE_INFINITY,
	opacity = 1,
	position,
	resolution = 512,
	scale = 10,
}: ContactShadowsProps) {
	const ref = React.useRef<Group>(null);
	const shadowCamera = React.useRef<OrthographicCamera>(null);
	const scene = useThree((state) => state.scene);
	const gl = useThree((state) => state.gl);
	const invalidate = useThree((state) => state.invalidate);
	const count = React.useRef(0);

	const res = React.useMemo(() => {
		const renderTarget = new WebGLRenderTarget(resolution, resolution);
		const renderTargetBlur = new WebGLRenderTarget(resolution, resolution);
		renderTarget.texture.generateMipmaps = false;
		renderTargetBlur.texture.generateMipmaps = false;
		const planeGeometry = new PlaneGeometry(scale, scale).rotateX(Math.PI / 2);
		const blurPlane = new Mesh(planeGeometry);
		const depthMaterial = new MeshDepthMaterial();
		depthMaterial.depthTest = false;
		depthMaterial.depthWrite = false;
		depthMaterial.onBeforeCompile = (shader) => {
			shader.uniforms.ucolor = { value: new Color(color) };
			shader.fragmentShader = shader.fragmentShader.replace(
				"void main() {",
				"uniform vec3 ucolor;\nvoid main() {",
			);
			// Colorize the shadow; the falloff keeps the centre darker.
			shader.fragmentShader = shader.fragmentShader.replace(
				"vec4( vec3( 1.0 - fragCoordZ ), opacity );",
				"vec4( ucolor * fragCoordZ * 2.0, ( 1.0 - fragCoordZ ) * 1.0 );",
			);
		};
		return {
			blurPlane,
			depthMaterial,
			horizontalBlur: blurMaterial(HorizontalBlurShader),
			planeGeometry,
			renderTarget,
			renderTargetBlur,
			verticalBlur: blurMaterial(VerticalBlurShader),
		};
	}, [resolution, scale, color]);

	React.useEffect(
		() => () => {
			res.renderTarget.dispose();
			res.renderTargetBlur.dispose();
			res.planeGeometry.dispose();
			res.depthMaterial.dispose();
			res.horizontalBlur.dispose();
			res.verticalBlur.dispose();
		},
		[res],
	);

	// New targets, bake settings or a bake request start from an empty count;
	// with an on-demand frameloop the bake also needs a frame scheduled.
	// biome-ignore lint/correctness/useExhaustiveDependencies: every input is a re-bake trigger
	React.useEffect(() => {
		count.current = 0;
		invalidate();
	}, [bakeKey, res, blur, far, invalidate]);

	const blurShadows = (amount: number, camera: OrthographicCamera) => {
		const { blurPlane, horizontalBlur, verticalBlur } = res;
		blurPlane.visible = true;
		blurPlane.material = horizontalBlur;
		horizontalBlur.uniforms.tDiffuse.value = res.renderTarget.texture;
		horizontalBlur.uniforms.h.value = amount / 256;
		gl.setRenderTarget(res.renderTargetBlur);
		gl.render(blurPlane, camera);
		blurPlane.material = verticalBlur;
		verticalBlur.uniforms.tDiffuse.value = res.renderTargetBlur.texture;
		verticalBlur.uniforms.v.value = amount / 256;
		gl.setRenderTarget(res.renderTarget);
		gl.render(blurPlane, camera);
		blurPlane.visible = false;
	};

	useFrame(() => {
		const camera = shadowCamera.current;
		const group = ref.current;
		if (!camera || !group || count.current >= frames) return;
		count.current++;
		const initialBackground = scene.background;
		const initialOverrideMaterial = scene.overrideMaterial;
		group.visible = false;
		scene.background = null;
		scene.overrideMaterial = res.depthMaterial;
		gl.setRenderTarget(res.renderTarget);
		gl.render(scene, camera);
		blurShadows(blur, camera);
		blurShadows(blur * 0.4, camera);
		gl.setRenderTarget(null);
		group.visible = true;
		scene.overrideMaterial = initialOverrideMaterial;
		scene.background = initialBackground;
	});

	return (
		<group position={position} ref={ref} rotation-x={Math.PI / 2}>
			<mesh
				geometry={res.planeGeometry}
				rotation={[-Math.PI / 2, 0, 0]}
				scale={[1, -1, 1]}
			>
				<meshBasicMaterial
					depthWrite={false}
					map={res.renderTarget.texture}
					opacity={opacity}
					transparent
				/>
			</mesh>
			<orthographicCamera
				args={[-scale / 2, scale / 2, scale / 2, -scale / 2, 0, far]}
				ref={shadowCamera}
			/>
		</group>
	);
}
