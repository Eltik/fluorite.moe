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
	alphaModeOf,
	describeGeometry,
	describeMaterial,
	describeMesh,
	describeMeshDetail,
	describeTextures,
	formatFloat,
	hexOf,
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

describe("describeGeometry", () => {
	test("lists attributes, UV sets and skinning", () => {
		const g = quad();
		g.setAttribute("uv", new BufferAttribute(new Float32Array(4 * 2), 2));
		g.setAttribute("uv1", new BufferAttribute(new Float32Array(4 * 2), 2));
		g.setAttribute("skinIndex", new BufferAttribute(new Uint16Array(4 * 4), 4));
		g.addGroup(0, 3, 0);
		g.addGroup(3, 3, 1);
		const d = describeGeometry(new Mesh(g));
		expect(d.vertices).toBe(4);
		expect(d.triangles).toBe(2);
		expect(d.indexed).toBe(true);
		expect(d.submeshes).toBe(2);
		expect(d.uvSets).toEqual(["uv", "uv1"]);
		expect(d.skinned).toBe(false);
		expect(d.bones).toBeNull();
		expect(d.attributes.find((a) => a.name === "skinIndex")).toEqual({
			name: "skinIndex",
			size: 4,
			type: "Uint16Array",
			normalized: false,
		});
	});

	test("counts one submesh for a plain geometry", () => {
		expect(describeGeometry(new Mesh(quad())).submeshes).toBe(1);
	});
});

describe("describeMaterial", () => {
	test("reads alpha mode, sides, queue and the extras tables", () => {
		const m = gameMaterial();
		m.alphaTest = 0.5;
		m.side = 2; // DoubleSide
		m.userData.renderQueue = 2450;
		m.userData.floats = { _Smoothness: 0.25, _EnableOutline: 1 };
		m.userData.ints = { _Cull: 2 };
		m.userData.keywords = ["_ALPHATEST_ON"];
		m.userData.colors = {
			_BaseColor: { r: 1, g: 0.5, b: 0, a: 1 },
			_RimColor: [2, 0.5, 0.5, 1],
		};
		const d = describeMaterial(m);
		expect(d.alphaMode).toBe("mask");
		expect(d.alphaCutoff).toBe(0.5);
		expect(d.doubleSided).toBe(true);
		expect(d.renderQueue).toBe(2450);
		expect(d.floats).toEqual([
			["_EnableOutline", 1],
			["_Smoothness", 0.25],
		]);
		expect(d.ints).toEqual([["_Cull", 2]]);
		expect(d.keywords).toEqual(["_ALPHATEST_ON"]);
		expect(d.colors.map((c) => c.name)).toEqual(["_BaseColor", "_RimColor"]);
		expect(d.colors[0].hex).toBe("#ff8000");
		expect(d.colors[0].hdr).toBe(false);
		expect(d.colors[1].hdr).toBe(true);
		expect(d.colors[1].rgba).toEqual([2, 0.5, 0.5, 1]);
	});

	test("handles plain materials and a missing one", () => {
		const m = new MeshStandardMaterial();
		m.transparent = true;
		expect(alphaModeOf(m)).toBe("blend");
		expect(alphaModeOf(new MeshStandardMaterial())).toBe("opaque");
		const d = describeMaterial(null);
		expect(d.shader).toBeNull();
		expect(d.floats).toEqual([]);
		expect(d.name).toBe("Unnamed material");
	});
});

describe("describeTextures", () => {
	test("reports size, colour space and UV set per slot", () => {
		const m = gameMaterial();
		(m.map as Texture).image = { width: 1024, height: 512 };
		(m.map as Texture).colorSpace = "srgb";
		m.userData.textures._SDFMap = { texture: 3, uvSet: 1 };
		m.userData.nprTextures._SDFMap.colorSpace = "";
		const slots = describeTextures(m);
		expect(slots.map((s) => s.slot)).toEqual(["_BaseMap", "_SDFMap"]);
		expect(slots[0]).toMatchObject({
			name: "T_chen_body_D",
			width: 1024,
			height: 512,
			colorSpace: "sRGB",
			uvSet: null,
		});
		expect(slots[1]).toMatchObject({ colorSpace: "Data", uvSet: 1 });
		expect(slots[1].width).toBeNull();
	});

	test("the full detail combines geometry, material and textures", () => {
		const d = describeMeshDetail(new Mesh(quad(), gameMaterial()));
		expect(d.material.shader).toBe("HGRP/CharacterNPR_Skin");
		expect(d.geometry.triangles).toBe(2);
		expect(d.textures).toHaveLength(2);
	});
});

describe("formatting", () => {
	test("hexOf clamps and pads", () => {
		expect(hexOf([1, 0, 0])).toBe("#ff0000");
		expect(hexOf([2, -1, 0.5])).toBe("#ff0080");
	});

	test("formatFloat keeps numbers short", () => {
		expect(formatFloat(1)).toBe("1");
		expect(formatFloat(0.123456)).toBe("0.1235");
		expect(formatFloat(0.5)).toBe("0.5");
		expect(formatFloat(12345.678)).toBe("1.23e+4");
		expect(formatFloat(0.00001)).toBe("1.00e-5");
	});
});

test("shortShader drops the game's folder", () => {
	expect(shortShader("HGRP/CharacterNPR_Hair")).toBe("CharacterNPR_Hair");
	expect(shortShader("Standard")).toBe("Standard");
});
