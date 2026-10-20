/**
 * The in-game character menu stage: an unlit textured dome and a floor quad
 * with signed-distance graph lines. The scene prefab is not exported, so the
 * node transforms and material constants are hard-coded here; the meshes and
 * textures come from our extracted assets. Dark theme draws it at 10 %.
 */
import { useThree } from "@react-three/fiber";
import * as React from "react";
import {
	BackSide,
	BufferGeometry,
	Float32BufferAttribute,
	type IUniform,
	LinearMipmapLinearFilter,
	Mesh,
	MeshBasicMaterial,
	Quaternion,
	RepeatWrapping,
	type Texture,
	Vector2,
	Vector3,
	Vector4,
	type WebGLProgramParametersWithUniforms,
} from "three";
import { MeshoptDecoder } from "three/examples/jsm/libs/meshopt_decoder.module.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { fetchBytes, isAbortError } from "@/components/studio/bytes";
import { loadTexture } from "@/components/studio/load-model";
import {
	FLOOR_FRAGMENT_PATCH,
	FLOOR_VERTEX_PATCH,
} from "@/components/studio/npr/shaders";
import { useSceneObjects } from "@/components/studio/npr/use-scene-objects";
import { fileUrl } from "@/lib/api";

const ASSETS = "beyond/arts/ui/sceneassets/charinfo";
const DOME_MESH = `${ASSETS}/models/GeoSphere001.glb`;
const WALL_TEXTURE = `${ASSETS}/textures/T_D_CharWall.png`;
const FLOOR_TEXTURE = `${ASSETS}/textures/T_D_CharFloor.png`;
const LINES_TEXTURE = `${ASSETS}/textures/deco_charInfo_floor_output_1.png`;

const DOME = {
	position: new Vector3(0.018, 0, 0.53),
	quaternion: new Quaternion(0.7041, 0.0647, -0.0647, 0.7041),
	scale: 0.75,
	tint: 0.917,
};
const FLOOR = {
	position: new Vector3(0.57, 0, 0.81),
	quaternion: new Quaternion(0.2126, -0.6744, 0.6744, 0.2126),
	scale: 35,
	tint: 0.9019,
	/** Line texture tiling (xy) and offset (zw). */
	lineSt: new Vector4(1, 1, 0.72, 0.45),
	/** View depths between which the lines switch from field to AA edge. */
	lineSwitch: new Vector2(30.7, 14.1),
	lineTint: new Vector4(1, 1, 1, 0.0431),
};
const RENDER_ORDER = -10;
const DARK_FACTOR = 0.1;

/** Unity's built-in Quad as the exporter writes it (x negated). */
function quadGeometry(): BufferGeometry {
	const g = new BufferGeometry();
	g.setAttribute(
		"position",
		new Float32BufferAttribute(
			[0.5, -0.5, 0, -0.5, -0.5, 0, 0.5, 0.5, 0, -0.5, 0.5, 0],
			3,
		),
	);
	g.setAttribute("uv", new Float32BufferAttribute([0, 1, 1, 1, 0, 0, 1, 0], 2));
	g.setAttribute(
		"normal",
		new Float32BufferAttribute([0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, -1], 3),
	);
	g.setIndex([1, 3, 0, 2, 0, 3]);
	return g;
}

interface FloorUniforms {
	uLineTex: IUniform<Texture>;
	uLineSt: IUniform<Vector4>;
	uLineSwitch: IUniform<Vector2>;
	uLineTint: IUniform<Vector4>;
	uStageFactor: IUniform<number>;
}

function floorMaterial(
	map: Texture,
	lines: Texture,
): { material: MeshBasicMaterial; uniforms: FloorUniforms } {
	const uniforms: FloorUniforms = {
		uLineTex: { value: lines },
		uLineSt: { value: FLOOR.lineSt },
		uLineSwitch: { value: FLOOR.lineSwitch },
		uLineTint: { value: FLOOR.lineTint },
		uStageFactor: { value: 1 },
	};
	const material = new MeshBasicMaterial({
		name: "stage floor",
		map,
		toneMapped: false,
	});
	material.onBeforeCompile = (shader: WebGLProgramParametersWithUniforms) => {
		Object.assign(shader.uniforms, uniforms);
		shader.vertexShader = `varying float vStageDepth;\n${shader.vertexShader.replace(
			"#include <project_vertex>",
			FLOOR_VERTEX_PATCH,
		)}`;
		shader.fragmentShader = `uniform sampler2D uLineTex;
uniform vec4 uLineSt;
uniform vec2 uLineSwitch;
uniform vec4 uLineTint;
uniform float uStageFactor;
varying float vStageDepth;
${shader.fragmentShader.replace("#include <map_fragment>", FLOOR_FRAGMENT_PATCH)}`;
	};
	material.customProgramCacheKey = () => "stage-floor";
	return { material, uniforms };
}

interface StageAssets {
	dome: Mesh;
	floor: Mesh;
	floorUniforms: FloorUniforms;
	dispose: () => void;
}

async function loadAssets(signal: AbortSignal): Promise<StageAssets> {
	const loader = new GLTFLoader();
	loader.setMeshoptDecoder(MeshoptDecoder);
	const [domeGltf, wall, floorMap, lines] = await Promise.all([
		fetchBytes(fileUrl("meshes", DOME_MESH), {
			signal,
			onProgress: () => {},
		}).then((buf) => loader.parseAsync(buf, "")),
		loadTexture(WALL_TEXTURE, { signal }),
		loadTexture(FLOOR_TEXTURE, { signal, srgb: false }),
		loadTexture(LINES_TEXTURE, { signal, srgb: false }),
	]);
	let domeGeometry: BufferGeometry | null = null;
	domeGltf.scene.traverse((node) => {
		const mesh = node as Mesh;
		if (mesh.isMesh && !domeGeometry) domeGeometry = mesh.geometry;
	});
	if (!domeGeometry) throw new Error("The stage dome mesh is empty.");

	for (const t of [wall, floorMap, lines]) {
		t.wrapS = RepeatWrapping;
		t.wrapT = RepeatWrapping;
	}
	wall.minFilter = LinearMipmapLinearFilter;
	floorMap.minFilter = LinearMipmapLinearFilter;

	const dome = new Mesh(
		domeGeometry,
		new MeshBasicMaterial({
			name: "stage dome",
			map: wall,
			side: BackSide,
			toneMapped: false,
		}),
	);
	dome.position.copy(DOME.position);
	dome.quaternion.copy(DOME.quaternion);
	dome.scale.setScalar(DOME.scale);
	dome.renderOrder = RENDER_ORDER;
	dome.frustumCulled = false;
	dome.raycast = () => {};

	const floorGeometry = quadGeometry();
	const { material, uniforms } = floorMaterial(floorMap, lines);
	const floor = new Mesh(floorGeometry, material);
	floor.position.copy(FLOOR.position);
	floor.quaternion.copy(FLOOR.quaternion);
	floor.scale.setScalar(FLOOR.scale);
	floor.renderOrder = RENDER_ORDER;
	floor.frustumCulled = false;
	floor.raycast = () => {};

	return {
		dome,
		floor,
		floorUniforms: uniforms,
		dispose: () => {
			dome.material.dispose();
			material.dispose();
			floorGeometry.dispose();
			domeGltf.scene.traverse((node) => {
				const mesh = node as Mesh;
				if (mesh.isMesh) mesh.geometry.dispose();
			});
			for (const t of [wall, floorMap, lines]) {
				t.dispose();
				(t.source.data as ImageBitmap | undefined)?.close?.();
			}
		},
	};
}

let cached: Promise<StageAssets> | null = null;

/** Loaded once per page; the controller only matters for the first load. */
function getAssets(signal: AbortSignal): Promise<StageAssets> {
	if (!cached) {
		cached = loadAssets(signal).catch((error: unknown) => {
			cached = null;
			throw error;
		});
	}
	return cached;
}

export function StageBackdrop({
	dark,
	floor,
}: {
	dark: boolean;
	/** Whether to draw the floor quad (the dome always shows). */
	floor: boolean;
}): null {
	const invalidate = useThree((s) => s.invalidate);
	const [assets, setAssets] = React.useState<StageAssets | null>(null);

	React.useEffect(() => {
		const controller = new AbortController();
		getAssets(controller.signal).then(
			(a) => {
				if (!controller.signal.aborted) setAssets(a);
			},
			(error: unknown) => {
				if (!isAbortError(error)) console.warn("Stage backdrop failed", error);
			},
		);
		return () => controller.abort();
	}, []);

	React.useEffect(() => {
		if (!assets) return;
		const factor = dark ? DARK_FACTOR : 1;
		(assets.dome.material as MeshBasicMaterial).color.setScalar(
			DOME.tint * factor,
		);
		(assets.floor.material as MeshBasicMaterial).color.setScalar(
			FLOOR.tint * factor,
		);
		assets.floorUniforms.uStageFactor.value = factor;
		invalidate();
	}, [assets, dark, invalidate]);

	const objects = React.useMemo(
		() => (assets ? (floor ? [assets.dome, assets.floor] : [assets.dome]) : []),
		[assets, floor],
	);
	useSceneObjects(objects);
	return null;
}
