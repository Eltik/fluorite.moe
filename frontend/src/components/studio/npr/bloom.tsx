/**
 * Optional post chain: render → bloom (threshold .92, strength .15, radius
 * .4) → +8 % saturation → sRGB output, at pixel ratio 1. The materials have
 * already tone mapped and clamped, so bloom only catches true highlights.
 */
import { useFrame, useThree } from "@react-three/fiber";
import * as React from "react";
import { HalfFloatType, Vector2, WebGLRenderTarget } from "three";
import { EffectComposer } from "three/examples/jsm/postprocessing/EffectComposer.js";
import { OutputPass } from "three/examples/jsm/postprocessing/OutputPass.js";
import { RenderPass } from "three/examples/jsm/postprocessing/RenderPass.js";
import { ShaderPass } from "three/examples/jsm/postprocessing/ShaderPass.js";
import { UnrealBloomPass } from "three/examples/jsm/postprocessing/UnrealBloomPass.js";

const BLOOM = { threshold: 0.92, strength: 0.15, radius: 0.4 };
const SATURATION = 1.08;

const saturationShader = {
	name: "StageSaturation",
	uniforms: {
		tDiffuse: { value: null },
		uAmount: { value: SATURATION },
	},
	vertexShader: /* glsl */ `
		varying vec2 vUv;
		void main() {
			vUv = uv;
			gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
		}
	`,
	fragmentShader: /* glsl */ `
		uniform sampler2D tDiffuse;
		uniform float uAmount;
		varying vec2 vUv;
		void main() {
			vec4 c = texture2D(tDiffuse, vUv);
			float l = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
			gl_FragColor = vec4(mix(vec3(l), c.rgb, uAmount), c.a);
		}
	`,
};

export function Bloom({ enabled }: { enabled: boolean }): null {
	const gl = useThree((s) => s.gl);
	const scene = useThree((s) => s.scene);
	const camera = useThree((s) => s.camera);
	const size = useThree((s) => s.size);
	const invalidate = useThree((s) => s.invalidate);

	const composer = React.useMemo(() => {
		if (!enabled) return null;
		const target = new WebGLRenderTarget(1, 1, {
			type: HalfFloatType,
			stencilBuffer: true,
			samples: 2,
		});
		const c = new EffectComposer(gl, target);
		c.addPass(new RenderPass(scene, camera));
		c.addPass(
			new UnrealBloomPass(
				new Vector2(1, 1),
				BLOOM.strength,
				BLOOM.radius,
				BLOOM.threshold,
			),
		);
		c.addPass(new ShaderPass(saturationShader));
		c.addPass(new OutputPass());
		c.setPixelRatio(1);
		return c;
	}, [enabled, gl, scene, camera]);

	React.useEffect(() => {
		composer?.setSize(size.width, size.height);
		invalidate();
	}, [composer, size, invalidate]);

	React.useEffect(() => () => composer?.dispose(), [composer]);

	// A positive priority takes over rendering from r3f while enabled.
	useFrame(
		() => {
			composer?.render();
		},
		enabled ? 1 : 0,
	);

	return null;
}
