/**
 * GLSL for the in-game character look (HGRP/CharacterNPR*): the tone curve,
 * the NPR surface shader, the inverted-hull outline and the stage floor's
 * line overlay. Everything here is uniform driven; three.js lights are not
 * used. See `material.ts` for how `extras` map onto the defines and uniforms.
 */

/**
 * The game's tone curve: linear sRGB → AP1, a rational ACES-like fit, 7 %
 * desaturation, back to sRGB and a hue-preserving highlight roll-off. Every
 * character material (and the outline and eyes) runs through it; the stage
 * does not. The renderer itself keeps `NoToneMapping`.
 */
export const TONE_CURVE_GLSL = /* glsl */ `
vec3 nprToneMap(vec3 c) {
	vec3 ap1 = max(vec3(
		dot(vec3(0.613192, 0.339512, 0.047366), c),
		dot(vec3(0.070206, 0.916336, 0.013450), c),
		dot(vec3(0.020619, 0.109568, 0.869606), c)), 0.0);
	vec3 fit = min(ap1 * (2.785085 * ap1 + 0.107772)
		/ max(ap1 * (2.936045 * ap1 + 0.887122) + 0.806889, 1e-4), 1.0);
	const vec3 lumAp1 = vec3(0.272229, 0.674082, 0.053689);
	float l = dot(lumAp1, fit);
	fit = l + 0.93 * (fit - l);
	vec3 srgb = vec3(
		dot(vec3(1.705052, -0.621791, -0.083259), fit),
		dot(vec3(-0.130257, 1.140803, -0.010548), fit),
		dot(vec3(-0.024003, -0.128969, 1.152972), fit));
	float roll = clamp((dot(lumAp1, ap1) - 0.5) / 1.5, 0.0, 1.0);
	vec3 hue = clamp(srgb / max(max(srgb.r, max(srgb.g, srgb.b)), 1e-5), 0.0, 1.0);
	return clamp(srgb + roll * (hue - srgb), 0.0, 1.0);
}
`;

const BASE_SAMPLE_GLSL = /* glsl */ `
vec4 sampleBase(vec2 uv) {
	vec4 base = vec4(uBaseColor, uOpacity);
	#ifdef HAS_MAP
	base *= texture2D(uMap, uv);
	#endif
	return base;
}
`;

export const NPR_VERTEX = /* glsl */ `
#include <common>
#include <skinning_pars_vertex>
varying vec2 vUv;
varying vec3 vNormalW;
varying vec3 vPosW;

void main() {
	vUv = uv;
	vec3 objectNormal = vec3(normal);
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <begin_vertex>
	#include <skinning_vertex>
	vec4 world = modelMatrix * vec4(transformed, 1.0);
	vPosW = world.xyz;
	vNormalW = normalize(mat3(modelMatrix) * objectNormal);
	gl_Position = projectionMatrix * viewMatrix * world;
}
`;

const NPR_FRAGMENT_HEAD = /* glsl */ `
#include <common>
uniform sampler2D uMap;
uniform vec3 uBaseColor;
uniform float uOpacity;
uniform float uAlphaTest;
uniform sampler2D uDiffRamp;
uniform sampler2D uSpecRamp;
uniform sampler2D uMetalGloss;
uniform vec4 uPbrConst;
uniform sampler2D uSdf;
uniform sampler2D uSdfMask;
uniform sampler2D uShadowLut;
uniform float uLutSize;
uniform float uShadowBrightness;
uniform float uShadowSaturation;
uniform float uSpecular;
uniform float uRimEnable;
uniform vec3 uSdfRimColor;
uniform float uSkinRimOffScale;
uniform float uFaceRimOffScale;
uniform sampler2D uHighlightMap;
uniform vec2 uHighlightVec;
uniform vec3 uHeadForward;
uniform vec3 uHeadRight;
uniform vec3 uHeadPos;
uniform float uAlphaPremul;
uniform sampler2D uEmissionMap;
uniform vec3 uEmissionColor;
// Shared light rig (see rig.ts).
uniform vec3 uLightDir;
uniform vec3 uLightColor;
uniform vec3 uSkinLightColor;
uniform vec3 uShadowTint;
uniform float uRangeBias;
uniform float uEnvMul;
uniform float uEnvShadowMul;
uniform vec3 uRimColor;
uniform float uRimWidth;
uniform float uSpecStrength;

// Additional (per-character) wrap and rim lights, packed per frame.
#define NPR_MAX_LIGHTS 16
uniform int uAddLightCount;
uniform vec4 uAddLightPos[NPR_MAX_LIGHTS];   // xyz, 1 / range
uniform vec4 uAddLightColor[NPR_MAX_LIGHTS]; // rgb, w ≥ 3 = rim light
uniform vec4 uAddLightDir[NPR_MAX_LIGHTS];   // forward xyz, falloff exponent (< 0 = inverse-square)
uniform vec4 uAddLightCone[NPR_MAX_LIGHTS];  // cos outer, 1 / (cos inner − cos outer), is spot
uniform vec4 uAddLightData[NPR_MAX_LIGHTS];  // wrap: amount, auto-limit; rim: width, albedo mix

varying vec2 vUv;
varying vec3 vNormalW;
varying vec3 vPosW;

#if defined(NPR_SKIN) && defined(USE_DIFF_RAMP)
#define SKIN_RAMP
#endif

${""}`;

/**
 * Depth-map lookup shared by the surface shader and the ground receiver. The
 * map is a comparison sampler (hardware PCF), filtered with a 5×5-texel tent
 * collapsed into 3×3 bilinear taps, which gives the soft painted penumbra
 * of the game instead of a stepped edge.
 */
const SELF_SHADOW_GLSL = /* glsl */ `
uniform highp sampler2DShadow uShadowMap;
uniform mat4 uShadowMatrix;
uniform vec2 uShadowBias;
uniform float uShadowTexel;
uniform float uShadowOn;

/** Area of the 5-texel tent (half-width 2.5, normalised to 1) left of x. */
float tentCdf(float x) {
	x = clamp(x, -2.5, 2.5);
	return (2.5 * x - 0.5 * x * abs(x) + 3.125) / 6.25;
}

/**
 * Collapses the five tent-weighted texels along one axis into three bilinear
 * taps (weights w, texel offsets o from the nearest centre) for a sample that
 * sits f texels (−0.5…0.5) from that centre.
 */
void tentTaps(float f, out vec3 w, out vec3 o) {
	float w2n = tentCdf(-1.5 - f) - tentCdf(-2.5 - f);
	float w1n = tentCdf(-0.5 - f) - tentCdf(-1.5 - f);
	float w0 = tentCdf(0.5 - f) - tentCdf(-0.5 - f);
	float w1p = tentCdf(1.5 - f) - tentCdf(0.5 - f);
	float w2p = tentCdf(2.5 - f) - tentCdf(1.5 - f);
	float wa = w2n + w1n;
	float wc = w1p + w2p;
	w = vec3(wa, w0, wc) / (wa + w0 + wc);
	o = vec3(-(2.0 * w2n + w1n) / max(wa, 1e-5), 0.0, (w1p + 2.0 * w2p) / max(wc, 1e-5));
}

/** 1 lit, 0 shadowed; outside the map everything is lit. */
float selfShadow(vec3 P, vec3 N, vec3 L) {
	if (uShadowOn < 0.5) return 1.0;
	float k = 1.0 - clamp(dot(N, L), 0.0, 0.9);
	vec3 p = P + L * (uShadowBias.x * k) + N * (uShadowBias.y * k);
	vec4 sc = uShadowMatrix * vec4(p, 1.0);
	vec3 c = sc.xyz / sc.w;
	if (c.x <= 0.0 || c.x >= 1.0 || c.y <= 0.0 || c.y >= 1.0 || c.z >= 1.0) return 1.0;
	float z = max(c.z, 0.0);
	// Texel-centre coordinates (centres on integers).
	vec2 t = c.xy / uShadowTexel - 0.5;
	vec2 base = floor(t + 0.5);
	vec2 f = t - base;
	vec3 wx;
	vec3 ox;
	vec3 wy;
	vec3 oy;
	tentTaps(f.x, wx, ox);
	tentTaps(f.y, wy, oy);
	float lit = 0.0;
	for (int j = 0; j < 3; j++) {
		for (int i = 0; i < 3; i++) {
			vec2 uv = (base + vec2(ox[i], oy[j]) + 0.5) * uShadowTexel;
			lit += wx[i] * wy[j] * texture(uShadowMap, vec3(uv, z));
		}
	}
	return lit;
}
`;

export const GROUND_SHADOW_VERTEX = /* glsl */ `
varying vec3 vPosW;
void main() {
	vec4 world = modelMatrix * vec4(position, 1.0);
	vPosW = world.xyz;
	gl_Position = projectionMatrix * viewMatrix * world;
}
`;

/** Multiply-blended receiver: darkens the floor where the character shadows it. */
export const GROUND_SHADOW_FRAGMENT = /* glsl */ `
uniform vec3 uLightDir;
uniform vec3 uCentre;
uniform float uInner;
uniform float uFade;
varying vec3 vPosW;
${SELF_SHADOW_GLSL}
void main() {
	float t = clamp((uInner + uFade - distance(vPosW, uCentre)) / uFade, 0.0, 1.0);
	float f = t * t * (3.0 - 2.0 * t);
	if (f <= 0.0) discard;
	float v = clamp(0.95 - selfShadow(vPosW, vec3(0.0, 1.0, 0.0), normalize(uLightDir)), 0.0, 1.0) * f;
	gl_FragColor = vec4(mix(vec3(1.0), vec3(0.4452), v), 1.0);
}
`;

const NPR_FRAGMENT_TAIL = /* glsl */ `
${SELF_SHADOW_GLSL}

const float AMBIENT = 0.2877;
const float SKIN_AMBIENT = 0.725;
const float ALBEDO_SCALE = 0.96;
const float SDF_BIAS = 0.0;
const float SDF_SOFTNESS = 0.03;
// Fake studio softbox: floor, ceiling, panel strength and panel direction.
const float ENV_FLOOR = 0.5;
const float ENV_CEILING = 2.0;
const float ENV_PANEL = 6.0;
const float ENV_PANEL_COS = 0.85;

${BASE_SAMPLE_GLSL}
${TONE_CURVE_GLSL}

float lum(vec3 c) { return dot(c, vec3(0.212673, 0.715152, 0.072175)); }

vec3 saturateColor(vec3 c, float s) {
	float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
	return max(mix(vec3(l), c, s), 0.0);
}

vec3 toSrgb(vec3 c) {
	c = clamp(c, 0.0, 1.0);
	return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}

vec3 fromSrgb(vec3 c) {
	return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c));
}

float smooth01(float x) {
	x = clamp(x, 0.0, 1.0);
	return x * x * (3.0 - 2.0 * x);
}

/** World direction of the camera's view-space Z axis (towards the viewer). */
vec3 cameraAxis() {
	return vec3(viewMatrix[0][2], viewMatrix[1][2], viewMatrix[2][2]);
}

/** Deep shadow: 35 % darker with 20 % more saturation. */
vec3 deepShadow(vec3 c) {
	vec3 d = 0.65 * c;
	float l = lum(d);
	return l + 1.2 * (d - l);
}

/** Tints by the ramp colour in proportion to its chroma; keeps luminance. */
vec3 rampTint(vec3 c, vec3 ramp) {
	float chroma = max(ramp.r, max(ramp.g, ramp.b)) - min(ramp.r, min(ramp.g, ramp.b));
	vec3 tinted = c * mix(vec3(1.0), ramp, chroma);
	return tinted * clamp(lum(c) / max(lum(tinted), 0.001), 0.0, 1.5);
}

#ifdef SHADOW_LUT
/** Skin shadow colour from an N×N² strip LUT, indexed in sRGB. */
vec3 lutShadow(vec3 albedo) {
	float n = uLutSize;
	vec3 c = toSrgb(albedo) * (n - 1.0);
	float b0 = floor(c.b);
	float b1 = min(b0 + 1.0, n - 1.0);
	float v = 1.0 - (c.g + 0.5) / n;
	vec3 s0 = texture2D(uShadowLut, vec2((b0 * n + c.r + 0.5) / (n * n), v)).rgb;
	vec3 s1 = texture2D(uShadowLut, vec2((b1 * n + c.r + 0.5) / (n * n), v)).rgb;
	return fromSrgb(mix(s0, s1, c.b - b0));
}
#endif

vec3 shadowColor(vec3 albedo) {
	#ifdef SHADOW_LUT
	return lutShadow(albedo);
	#else
	return saturateColor(albedo, uShadowSaturation) * uShadowBrightness;
	#endif
}

#ifdef FACE_SDF
/** Mirrors the face UV when the light comes from the head's right side. */
vec2 sdfUv(float side) {
	return side < 0.0 ? vUv : vec2(1.0 - vUv.x, vUv.y);
}

/** SDF.b as a threshold against the light's yaw around the head: 0..1 lit. */
float sdfThreshold(vec3 L) {
	float f = dot(L, uHeadForward);
	float s = dot(L, uHeadRight);
	float len = length(vec2(f, s));
	float angle = len < 1e-4 ? 0.0 : acos(clamp(f / len, -1.0, 1.0)) / PI;
	float d = texture2D(uSdf, sdfUv(s)).b;
	return smoothstep(angle - SDF_SOFTNESS, angle + SDF_SOFTNESS, d);
}

/** (SDF.r + SDF.g) / 2 turned into a pseudo N·L for the skin ramp. */
float sdfRampInput(vec3 L) {
	float f = dot(L, uHeadForward);
	float s = dot(L, uHeadRight);
	vec2 l = vec2(s, f) / max(length(vec2(s, f)), 1e-4);
	float t = clamp(l.y + SDF_BIAS, -1.0, 1.0);
	vec4 m = texture2D(uSdf, sdfUv(s));
	float d = 0.5 * (m.r + m.g);
	float e = clamp(0.5 - 0.5 * t, 0.001, 0.999);
	float k = smoothstep(max(2.0 * e - 1.0, 0.0), min(2.0 * e, 1.0), d);
	return 2.0 * k - 1.0 + max(t, 0.0);
}
#endif

/**
 * Wrap lights add tinted fill (albedo × wrapped N·L); rim lights add a band
 * on the silhouette facing the light, gated to the lit hemisphere.
 */
vec3 additionalLights(vec3 N, vec3 wrapN, vec3 sideN, vec3 V, vec3 wrapAlbedo,
		vec3 rimAlbedo, float faceFront, float faceRim) {
	vec3 sum = vec3(0.0);
	vec3 Z = cameraAxis();
	float edge = 1.0 - abs(dot(N, V));
	#ifdef SKIN_RAMP
	const float wrapBase = 0.5;
	#else
	const float wrapBase = 0.25;
	#endif
	for (int i = 0; i < NPR_MAX_LIGHTS; i++) {
		if (i >= uAddLightCount) break;
		vec4 P = uAddLightPos[i];
		vec4 D = uAddLightDir[i];
		vec4 cone = uAddLightCone[i];
		vec4 data = uAddLightData[i];
		vec3 C = uAddLightColor[i].rgb;
		vec3 toLight = P.xyz - vPosW;
		float d2 = max(dot(toLight, toLight), 1e-8);
		vec3 L = toLight * inversesqrt(d2);
		float t = d2 * P.w * P.w;
		float A;
		if (D.w < 0.0) {
			float w = max(1.0 - t * t, 0.0);
			A = w * w / (d2 + 1.0);
		} else {
			A = pow(max(1.0 - min(t, 1.0), 0.0), D.w);
		}
		if (cone.z > 0.5) {
			float c = clamp((dot(L, -D.xyz) - cone.x) * cone.y, 0.0, 1.0);
			A *= c * c;
		}
		if (A <= 0.0001) continue;
		if (uAddLightColor[i].w < 0.5) {
			vec3 AC = A * C;
			float limit = (1.0 - data.y) + data.y / max(max(AC.r, max(AC.g, AC.b)) * 0.75, 1.0);
			float wrap = wrapBase * data.x + (1.0 - wrapBase * data.x) * clamp(dot(wrapN, L) + 0.5, 0.0, 1.0);
			sum += AC * limit * wrap * wrapAlbedo;
		} else {
			vec3 side = cross(Z, cross(Z, L));
			float sideLen = length(side);
			float facing = sideLen > 1e-5 ? clamp(dot(N, -side / sideLen), 0.0, 1.0) : 0.0;
			float hemi = clamp(dot(sideN, L) + 1.0, 0.0, 1.0);
			float band = smooth01((edge - (0.8 - 0.6 * data.x)) / (0.1 + 0.2 * data.x));
			#ifdef SKIN_RAMP
			float k = mix(band * faceFront, faceFront * faceRim, clamp(10.0 * data.x - 3.0, 0.0, 1.0)) * hemi;
			#else
			float k = band * hemi;
			#endif
			sum += A * k * C * facing * (0.5 + data.y * (rimAlbedo - 0.5));
		}
	}
	return sum;
}

/** How frontal the head is to the camera (0 off-axis, 1 within ~25°). */
float headFrontality() {
	vec3 Z = cameraAxis();
	vec2 zf = vec2(dot(Z, uHeadRight), dot(Z, uHeadForward));
	float zy = zf.y / max(length(zf), 1e-4);
	return smooth01((abs(zy) - 0.9) * 10.0);
}

#ifdef PBR_CLOTH
/** Softbox environment: brighter towards the ceiling plus a key-side panel. */
vec3 fakeEnvironment(vec3 R, float rough, vec3 L) {
	float w = 0.05 + 0.6 * rough * rough;
	float ambient = mix(ENV_FLOOR, ENV_CEILING, smoothstep(-0.5 - w, 0.5 + w, R.y));
	vec3 panelDir = normalize(vec3(L.x, max(L.y, 0.3), L.z));
	float panel = smoothstep(ENV_PANEL_COS - w, ENV_PANEL_COS + 0.5 * w, dot(R, panelDir))
		* ENV_PANEL / (1.0 + 6.0 * rough * rough);
	return vec3(ambient + panel);
}

/** Rational fit of the split-sum environment BRDF (Karis). */
vec3 environmentBrdf(vec3 f0, float rough, float nv) {
	float r2 = rough * rough;
	float r6 = r2 * r2 * r2;
	float v2 = nv * nv;
	float v3 = v2 * nv;
	float x = (0.036546 + 3.32707 * nv + (9.0632 - 9.04756 * nv) * r2)
		/ (1.0 + 3.59685 * v2 - 1.36772 * v3
			+ (9.04401 - 16.3174 * v2 + 9.22949 * v3) * r2
			+ (5.56589 + 19.7886 * v2 - 20.2123 * v3) * r6);
	float y = (0.99044 - 1.28514 * nv + (1.29678 - 0.755907 * nv) * r2)
		/ (1.0 + 2.92338 * nv + 59.4188 * v3
			+ (20.3225 - 27.0302 * nv + 222.592 * v3) * r2
			+ (121.563 + 626.13 * nv + 316.627 * v3) * r6);
	vec3 brdf = f0 * x + y;
	float e = x + y;
	return brdf * (1.0 + f0 * (1.0 - e) / e);
}
#endif

void main() {
	vec4 base = sampleBase(vUv);
	#ifdef ALPHA_TEST
	if (base.a < uAlphaTest) discard;
	#endif

	vec3 N = normalize(vNormalW);
	#ifdef DOUBLE_SIDED
	N *= gl_FrontFacing ? 1.0 : -1.0;
	#endif
	vec3 L = normalize(uLightDir);
	vec3 V = normalize(cameraPosition - vPosW);
	vec3 H = normalize(L + V);
	float NdotL = dot(N, L);
	float NdotV = clamp(dot(N, V), 0.0, 1.0);
	float NdotH = clamp(dot(N, H), 0.0, 1.0);
	#if defined(HAS_P_MAP)
	// r = metallic, g = specular, b = shadow/AO mask, a = smoothness.
	vec4 P = texture2D(uMetalGloss, vUv);
	#elif defined(P_CONST)
	// No P map embedded: the material constants stand in for it.
	vec4 P = uPbrConst;
	#endif
	// 1 lit, 0 in the character's own shadow.
	float sh = selfShadow(vPosW, N, L);

	vec3 color;
	vec3 light;
	float litK;
	// Inputs for the additional lights (set per branch).
	vec3 wrapN = N;
	vec3 sideN = N;
	vec3 wrapAlbedo;
	vec3 rimAlbedo = base.rgb * ALBEDO_SCALE;
	float faceFront = 0.0;
	float faceRim = 0.0;

#ifdef SKIN_RAMP
	// ---- skin with a ramp (faces, bodies)
	float x = clamp(NdotL + uRangeBias, -1.0, 1.0);
	float rimOff = uSkinRimOffScale;
	#ifdef FACE_FLAT
	// No SDF embedded: shade the face around a cylinder through the head,
	// which is what the SDF stylises, instead of the geometric normal.
	{
		vec3 headOffset = vPosW - uHeadPos;
		vec3 flatN = normalize(vec3(headOffset.x, 0.0, headOffset.z));
		x = clamp(dot(flatN, L) + uRangeBias, -1.0, 1.0);
	}
	#endif
	#ifdef FACE_SDF
	vec4 mask = vec4(1.0);
	#ifdef SDF_MASK
	mask = texture2D(uSdfMask, vUv);
	#endif
	// mask.g: 0 = SDF face shading, 1 = regular N·L.
	x = mix(sdfRampInput(L), x, mask.g);
	rimOff = mix(uFaceRimOffScale, uSkinRimOffScale, mask.b) * mask.r;
	#endif
	float rimT = clamp(0.85 * (1.0 - NdotV) * rimOff, 0.0, 1.0);
	vec4 ramp = texture2D(uDiffRamp, vec2(clamp(x * 0.5 + 0.5, 0.002, 0.998), 0.5));
	// Self-shadow fades out on the face as the view turns frontal; the
	// albedo alpha doubles as a baked shadow mask on skin.
	float shadowLit = 1.0;
	float shadowFill = 1.0;
	#ifdef FACE_FLAT
	// Without the SDF mask the whole face counts as SDF-shaded: no self-shadow.
	shadowLit = 0.0;
	shadowFill = 0.0;
	#endif
	#ifdef FACE_SDF
	{
		vec3 Zc = cameraAxis();
		vec2 zf = vec2(dot(Zc, uHeadRight), dot(Zc, uHeadForward));
		float frontal = smooth01(1.5 - 2.0 * zf.y / max(length(zf), 1e-4));
		shadowLit = max(mask.b * frontal, mask.g);
		shadowFill = mask.g;
	}
	#endif
	float s = mix(1.0, sh, shadowLit);
	litK = min(base.a, min(s, ramp.a));
	float fillK = base.a * mix(1.0, s, shadowFill);
	vec3 shadowCol = shadowColor(base.rgb) * ALBEDO_SCALE * uEnvMul;
	vec3 litCol = base.rgb * mix(vec3(1.0), uSdfRimColor, rimT) * ALBEDO_SCALE;
	vec3 c = mix(deepShadow(shadowCol), shadowCol, clamp(fillK + ramp.a, 0.0, 1.0));
	c = mix(c, litCol, litK);
	color = rampTint(c, ramp.rgb);
	light = mix(uShadowTint, vec3(1.0), litK) * SKIN_AMBIENT * AMBIENT * uSkinLightColor
		+ mix(vec3(lum(uSkinLightColor)), uSkinLightColor, litK);
	wrapAlbedo = color;
	color *= light;
	#ifdef HIGHLIGHT
	vec2 huv = vUv + vec2(-dot(V, uHeadRight), 0.0) * uHighlightVec.x;
	color += texture2D(uHighlightMap, huv).rgb * light * mix(uEnvMul, 1.0, litK) * (0.5 + 0.5 * litK);
	#endif
	// Faces take extra light around a cylinder through the head.
	vec4 addMask = vec4(1.0, 1.0, 1.0, 0.0);
	#ifdef FACE_SDF
	addMask = mask;
	vec3 headOffset = vPosW - uHeadPos;
	sideN = normalize(vec3(headOffset.x, 0.000061, headOffset.z));
	#endif
	faceFront = headFrontality();
	faceRim = addMask.a;
	wrapN = normalize(mix(sideN, N, addMask.g));
#else
	// ---- hair, cloth and skin without a ramp
	float x01 = clamp(NdotL + uRangeBias, -1.0, 1.0) * 0.5 + 0.5;
	#ifdef FACE_SDF
	float sdfMix = 1.0;
	#ifdef SDF_MASK
	sdfMix = 1.0 - texture2D(uSdfMask, vUv).g;
	#endif
	x01 = mix(x01, sdfThreshold(L), sdfMix);
	#endif
	vec3 shadowCol = shadowColor(base.rgb) * ALBEDO_SCALE * uEnvMul;
	float hemi = clamp(N.y + 0.15, 0.0, 1.0) * 1.5 + 0.5;

	#ifdef USE_DIFF_RAMP
	float pMask = 1.0;
	#if defined(HAS_P_MAP) || defined(P_CONST)
	pMask = P.b;
	#endif
	vec4 ramp = texture2D(uDiffRamp, vec2(clamp(x01, 0.002, 0.998), 0.5));
	// Front fill: the ramp read with N·V instead of N·L.
	float frontA = texture2D(uDiffRamp, vec2(clamp(dot(N, V) * 0.5 + 0.5, 0.002, 0.998), 0.5)).a;
	litK = min(ramp.a, min(pMask, sh));
	float fill = frontA * pMask * sh;
	vec3 litCol = base.rgb * ALBEDO_SCALE;
	vec3 c = mix(deepShadow(shadowCol), shadowCol, clamp(ramp.a + fill, 0.0, 1.0));
	c = mix(c, litCol, litK);
	color = rampTint(c, ramp.rgb);
	#elif defined(PBR_CLOTH)
	vec4 g = uPbrConst;
	#if defined(HAS_P_MAP) || defined(P_CONST)
	g = P;
	#endif
	float k = ALBEDO_SCALE * (1.0 - g.r);
	float s = smooth01((clamp(NdotL + uRangeBias, -1.0, 1.0) - 0.25) * 1.333333);
	vec3 Z = cameraAxis();
	float vf = smooth01((dot(N, Z) - 0.25) * 1.333333);
	litK = min(s, min(g.b, sh));
	float fill = vf * g.b * sh;
	vec3 shadowC = shadowCol * (1.0 - g.r);
	vec3 litCol = base.rgb * k;
	vec3 c = mix(deepShadow(shadowC), shadowC, clamp(s + fill, 0.0, 1.0));
	color = mix(c, litCol, litK);
	#else
	litK = min(smoothstep(0.48, 0.52, x01), sh);
	color = mix(shadowCol, base.rgb, litK);
	#endif

	light = mix(uShadowTint, vec3(1.0), litK) * hemi * AMBIENT * uLightColor
		+ mix(vec3(lum(uLightColor)), uLightColor, litK);
	wrapAlbedo = color;
	color *= light;
	#if defined(PREMUL) && !defined(PBR_CLOTH)
	color *= base.a;
	#endif

	#ifdef PBR_CLOTH
	rimAlbedo = litCol;
	float aK = mix(1.0, base.a, uAlphaPremul);
	color *= aK;
	float rough = 1.0 - g.a;
	float a = max(rough * rough, 0.007812);
	float a2 = a * a;
	// Half vector skewed towards the camera so the highlight reads from the front.
	vec3 Hs = normalize(L + 2.0 * normalize(vec3(Z.x, L.y, Z.z)) + 3.0 * V);
	float nh = dot(N, Hs);
	float dd = nh * nh * (a2 - 1.0) + 1.0;
	dd *= dd;
	float D = dd != a2 ? a2 / dd : 1.0;
	float nv = clamp(dot(N, V), 0.0, 1.0);
	float spec = clamp(D * 0.5 / (2.0 * nv + a + 0.0001) - 0.000061, 0.0, 20.0);
	vec3 f0 = mix(vec3(0.04 * g.g), base.rgb, g.r);
	float shadowEnv = mix(uEnvMul, 1.0, litK);
	color += light * (0.5 + 0.5 * litK) * shadowEnv * spec * f0;
	float lu = lum(color);
	float sat = clamp(lu - 0.5, 0.0, 0.5);
	color = lu + (color - lu) * (sat * sat + 1.0);
	#ifdef EMISSION
	color += texture2D(uEmissionMap, vUv).rgb * uEmissionColor * aK;
	#endif
	vec3 R = reflect(-V, N);
	color += fakeEnvironment(R, rough, L) * environmentBrdf(f0, rough, nv)
		* clamp(AMBIENT, 0.5, 1.5) * uEnvShadowMul * shadowEnv * uShadowTint;
	#endif
#endif

	color += additionalLights(N, wrapN, sideN, V, wrapAlbedo, rimAlbedo, faceFront, faceRim);

	#if defined(NPR_CLOTH) && defined(USE_METAL_GLOSS) && !defined(PBR_CLOTH)
	// Toon specular: a binarised Blinn lobe tinted by the spec ramp.
	float sm = P.a;
	float sp = pow(NdotH, exp2(mix(3.0, 9.0, sm))) * sm;
	sp = smoothstep(0.35, 0.6, sp) * step(0.0, NdotL);
	vec3 tint = vec3(1.0);
	#ifdef USE_SPEC_RAMP
	tint = texture2D(uSpecRamp, vec2(NdotH, 1.0 - sm)).rgb;
	#endif
	tint = mix(tint, tint * base.rgb * 2.0, P.r);
	color += sp * tint * uSpecular * uSpecStrength * uLightColor;
	#endif
	#if defined(NPR_HAIR) && defined(USE_SPEC_RAMP)
	// Hair: a band from the spec ramp along N·H.
	vec3 band = texture2D(uSpecRamp, vec2(NdotH, 0.5)).rgb;
	color += band * base.rgb * uSpecStrength * smoothstep(0.0, 0.2, NdotL) * uLightColor;
	#endif

	// The built-in rim only stands in while no additional lights are active.
	float rim = smoothstep(1.0 - uRimWidth, 1.0, 1.0 - NdotV) * uRimEnable
		* (uAddLightCount > 0 ? 0.0 : 1.0);
	color += rim * uRimColor * mix(vec3(1.0), base.rgb, 0.5);
	#if defined(NPR_CLOTH) && !defined(PBR_CLOTH) && defined(EMISSION)
	color += texture2D(uEmissionMap, vUv).rgb * uEmissionColor * mix(1.0, base.a, uAlphaPremul);
	#endif

	#ifdef TRANSPARENT
	gl_FragColor = vec4(color, base.a);
	#else
	gl_FragColor = vec4(color, 1.0);
	#endif
	gl_FragColor.rgb = nprToneMap(gl_FragColor.rgb);
	#include <colorspace_fragment>
}
`;

export const NPR_FRAGMENT = NPR_FRAGMENT_HEAD + NPR_FRAGMENT_TAIL;

export const OUTLINE_VERTEX = /* glsl */ `
#include <common>
#include <skinning_pars_vertex>
attribute vec3 smoothNormal;
uniform float uOutlineWidth;
uniform float uOutlineScale;
uniform float uOutlineOffsetZ;
uniform sampler2D uOutlineMask;
varying vec2 vUv;

void main() {
	vUv = uv;
	vec3 objectNormal = smoothNormal;
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <begin_vertex>
	#include <skinning_vertex>
	vec4 mvPosition = modelViewMatrix * vec4(transformed, 1.0);
	vec3 n = normalize(normalMatrix * objectNormal);
	float width = uOutlineWidth * uOutlineScale;
	#ifdef OUTLINE_MASK
	width *= texture2D(uOutlineMask, uv).r;
	#endif
	// Constant on screen between 0.4 m and 5 m of view depth.
	float depth = clamp(-mvPosition.z, 0.4, 5.0);
	mvPosition.xyz += n * width * depth / projectionMatrix[1][1];
	mvPosition.z -= uOutlineOffsetZ * 0.01;
	gl_Position = projectionMatrix * mvPosition;
}
`;

export const OUTLINE_FRAGMENT = /* glsl */ `
uniform sampler2D uMap;
uniform vec3 uBaseColor;
uniform float uOpacity;
uniform float uAlphaTest;
uniform float uOutlineBrightness;
uniform float uOutlineSaturation;
varying vec2 vUv;

${BASE_SAMPLE_GLSL}
${TONE_CURVE_GLSL}

vec3 saturateColor(vec3 c, float s) {
	float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
	return max(mix(vec3(l), c, s), 0.0);
}

void main() {
	vec4 base = sampleBase(vUv);
	#ifdef ALPHA_TEST
	if (base.a < uAlphaTest) discard;
	#endif
	gl_FragColor = vec4(saturateColor(base.rgb, uOutlineSaturation) * uOutlineBrightness, 1.0);
	gl_FragColor.rgb = nprToneMap(gl_FragColor.rgb);
	#include <colorspace_fragment>
}
`;

/** Vertex patch for the stage floor: view depth for the line switch. */
export const FLOOR_VERTEX_PATCH = /* glsl */ `
#include <project_vertex>
vStageDepth = -mvPosition.z;
`;

/**
 * Fragment patch for the stage floor, inserted after `map_fragment`: graph
 * lines from a signed-distance texture. Far away the field itself is drawn
 * softly; close up it becomes a pixel-exact anti-aliased edge.
 */
export const FLOOR_FRAGMENT_PATCH = /* glsl */ `
#include <map_fragment>
{
	vec2 st = vec2(vMapUv.x, 1.0 - vMapUv.y) * uLineSt.xy + uLineSt.zw;
	vec4 s = texture2D(uLineTex, vec2(st.x, 1.0 - st.y));
	float e = 0.5 - s.r;
	float grad = max(length(vec2(dFdx(e), dFdy(e))), 1e-6);
	float k = clamp((abs(vStageDepth) - uLineSwitch.x) / (uLineSwitch.y - uLineSwitch.x), 0.0, 1.0);
	float line = clamp(0.5 - mix(e, e / grad, k), 0.0, 1.0);
	float w = clamp((diffuseColor.a + s.a) * uLineTint.a, 0.0, 1.0);
	diffuseColor.rgb += line * uLineTint.rgb * w * s.a * uStageFactor;
}
`;
