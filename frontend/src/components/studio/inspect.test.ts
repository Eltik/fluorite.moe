import { describe, expect, test } from "bun:test";
import {
	BufferAttribute,
	BufferGeometry,
	Group,
	Mesh,
	MeshStandardMaterial,
	Texture,
} from "three";
import {
	describeMesh,
	partOf,
	shortShader,
	textureSlotsOf,
	triangleCountOf,
} from "@/components/studio/inspect";

function quad(): BufferGeometry {
	const g = new BufferGeometry();
	g.setAttribute("position", new BufferAttribute(new Float32Array(4 * 3), 3));
	g.setIndex([0, 1, 2, 0, 2, 3]);
	return g;
}

function gameMaterial(): MeshStandardMaterial {
	const m = new MeshStandardMaterial();
	m.name = "mat_chen_body";
	const base = new Texture();
	base.name = "T_chen_body_D";
	m.map = base;
	m.userData = {
		shader: "HGRP/CharacterNPR_Skin",
		textures: {
			_BaseMap: { texture: 0 },
			_SDFMap: { texture: 3 },
			_RampMap: { texture: null },
		},
		nprTextures: { _SDFMap: Object.assign(new Texture(), { name: "sdf" }) },
	};
	return m;
}

describe("describeMesh", () => {
	test("reads the game shader, counts and texture slots", () => {
		const mesh = new Mesh(quad(), gameMaterial());
		mesh.name = "chen_body";
		const root = new Group();
		const node = new Group();
		node.name = "chr_postmodel";
		root.add(node);
		node.add(mesh);
		const info = describeMesh(mesh, 0, root);
		expect(info.name).toBe("chen_body");
		expect(info.path).toEqual(["chr_postmodel"]);
		expect(info.shader).toBe("HGRP/CharacterNPR_Skin");
		expect(info.materialName).toBe("mat_chen_body");
		expect(info.triangles).toBe(2);
		expect(info.vertices).toBe(4);
		expect(info.skinned).toBe(false);
		expect(info.textures.map((t) => t.slot)).toEqual(["_BaseMap", "_SDFMap"]);
		expect(info.textures[0].detail).toBe("T_chen_body_D");
		expect(info.textures[1].detail).toBe("sdf");
	});

	test("uses the loader's material when a shading variant is active", () => {
		const original = gameMaterial();
		const mesh = new Mesh(quad(), new MeshStandardMaterial());
		mesh.userData.studioShading = { standard: original, renderOrder: 0 };
		expect(describeMesh(mesh).shader).toBe("HGRP/CharacterNPR_Skin");
	});

	test("clamps the material index on multi-material meshes", () => {
		const a = new MeshStandardMaterial();
		a.name = "a";
		const b = new MeshStandardMaterial();
		b.name = "b";
		const mesh = new Mesh(quad(), [a, b]);
		expect(describeMesh(mesh, 1).materialName).toBe("b");
		expect(describeMesh(mesh, 1).materialCount).toBe(2);
		expect(describeMesh(mesh, 7).materialIndex).toBe(1);
	});

	test("falls back to three's maps for plain GLBs", () => {
		const m = new MeshStandardMaterial();
		m.normalMap = new Texture();
		expect(textureSlotsOf(m)).toEqual([
			{ slot: "_BumpMap", detail: "embedded" },
		]);
		expect(describeMesh(new Mesh(quad(), m)).shader).toBeNull();
	});

	test("counts non-indexed geometry too", () => {
		const g = new BufferGeometry();
		g.setAttribute("position", new BufferAttribute(new Float32Array(9 * 3), 3));
		expect(triangleCountOf(new Mesh(g))).toBe(3);
	});
});

describe("partOf", () => {
	test("finds the part that owns a nested mesh", () => {
		const scene = new Group();
		const mesh = new Mesh(quad());
		scene.add(mesh);
		const other = new Group();
		const model = {
			parts: [
				{ id: "other", object: other },
				{ id: "scene", object: scene },
			],
		} as unknown as Parameters<typeof partOf>[0];
		expect(partOf(model, mesh)?.id).toBe("scene");
		expect(partOf(model, scene)?.id).toBe("scene");
		expect(partOf(model, new Group())).toBeNull();
	});
});

test("shortShader drops the game's folder", () => {
	expect(shortShader("HGRP/CharacterNPR_Hair")).toBe("CharacterNPR_Hair");
	expect(shortShader("Standard")).toBe("Standard");
});
