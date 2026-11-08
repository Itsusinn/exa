shader_type spatial;
render_mode diffuse_burley, specular_schlick_ggx;

uniform sampler2D wood_color : source_color, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D wood_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D wood_rough : filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D stone_color : source_color, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D stone_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D stone_rough : filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D grass_color : source_color, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D grass_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D grass_rough : filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D bark_color : source_color, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D bark_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D bark_rough : filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D granular_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform float normal_strength : hint_range(0.0,1.5) = 1.0;
varying vec3 world_position;
varying vec3 world_normal;
varying vec4 corner_ao;
varying vec4 exposed_edges;
uniform float contact_ao_strength : hint_range(0.0,1.0) = 0.85;
uniform float bevel_width : hint_range(0.0,0.1) = 0.035;
uniform float material_variation : hint_range(0.0,0.3) = 0.08;
// Two coherent samples for organic surfaces only. Masonry courses and wood
// grain retain their authored direction. Set to zero for same-camera review.
uniform bool organic_detiling = true;
// Development comparison: zero restores the complete phase-two surface response.
uniform float surface_detail_strength : hint_range(0.0,1.0) = 1.0;

vec3 srgb_to_linear(vec3 color) {
    return mix(color / 12.92, pow((color + 0.055) / 1.055, vec3(2.4)),
               step(vec3(0.04045), color));
}

float hash_cell(vec3 cell) {
    vec3 p = fract(cell * 0.1031);
    p += dot(p, p.yzx + 33.33);
    return fract((p.x + p.y) * p.z);
}

float value_noise(vec3 p) {
    vec3 cell = floor(p);
    vec3 f = fract(p);
    f = f*f*(3.0-2.0*f);
    return mix(mix(mix(hash_cell(cell),hash_cell(cell+vec3(1,0,0)),f.x),
                   mix(hash_cell(cell+vec3(0,1,0)),hash_cell(cell+vec3(1,1,0)),f.x),f.y),
               mix(mix(hash_cell(cell+vec3(0,0,1)),hash_cell(cell+vec3(1,0,1)),f.x),
                   mix(hash_cell(cell+vec3(0,1,1)),hash_cell(cell+vec3(1,1,1)),f.x),f.y),f.z);
}

vec3 organic_sample(sampler2D map, vec2 coords, float patch) {
    if (!organic_detiling) return texture(map,coords).rgb;
    float index = floor(patch);
    vec2 a = vec2(hash_cell(vec3(index,1,0)),hash_cell(vec3(index,2,0))) * 8.0;
    vec2 b = vec2(hash_cell(vec3(index+1.0,1,0)),hash_cell(vec3(index+1.0,2,0))) * 8.0;
    // Explicit derivatives ignore patch offsets at index boundaries. All PBR
    // channels use identical coordinates/weights, including the normal map.
    vec2 dx = dFdx(coords);
    vec2 dy = dFdy(coords);
    return mix(textureGrad(map,coords+a,dx,dy).rgb,
               textureGrad(map,coords+b,dx,dy).rgb,smoothstep(0.2,0.8,fract(patch)));
}

void vertex() {
    world_position = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
    world_normal = normalize(MODEL_NORMAL_MATRIX * NORMAL);
    // Decode before interpolation. Each face carries the same four samples on
    // both triangles, so the diagonal cannot introduce a lighting crease.
    int packed_ao = int(round(UV2.y));
    corner_ao = vec4(float(packed_ao & 3),float((packed_ao >> 2) & 3),
                     float((packed_ao >> 4) & 3),float((packed_ao >> 6) & 3));
    exposed_edges = vec4(float((packed_ao >> 8) & 1),float((packed_ao >> 9) & 1),
                         float((packed_ao >> 10) & 1),float((packed_ao >> 11) & 1));
}

// Continuous world-space variation: no per-block tint seams or new assets.
float material_noise(vec3 p) {
    return (value_noise(p)*0.7 + value_noise(p*2.03+vec3(17.0))*0.3)*2.0-1.0;
}

vec3 bevel_normal(vec3 position, vec3 normal, vec2 face_uv) {
    vec3 dx = dFdx(position);
    vec3 dy = dFdy(position);
    vec2 ux = dFdx(face_uv);
    vec2 uy = dFdy(face_uv);
    vec3 t = cross(dy,normal)*ux.x + cross(normal,dx)*uy.x;
    vec3 b = cross(dy,normal)*ux.y + cross(normal,dx)*uy.y;
    t *= inversesqrt(max(dot(t,t),1e-20));
    b *= inversesqrt(max(dot(b,b),1e-20));
    float width = max(bevel_width,0.00001);
    vec4 distance_to_edge = vec4(face_uv.y,1.0-face_uv.x,1.0-face_uv.y,face_uv.x);
    vec4 weight = (1.0-smoothstep(vec4(0.0),vec4(width),distance_to_edge))*exposed_edges;
    // Fade subpixel bevels instead of introducing distant glints.
    float footprint = max(length(dFdx(face_uv)),length(dFdy(face_uv)));
    weight *= (1.0-smoothstep(width,width*3.0,footprint))*float(bevel_width > 0.0);
    return normalize(normal + t*(weight.y-weight.w) + b*(weight.z-weight.x));
}

// Derive a tangent frame from the actual surface projection. Chunk meshes do
// not carry tangents; this also keeps rotated actor/item meshes consistent.
vec3 surface_normal(vec3 sample_normal, vec2 coords, vec3 position, vec3 normal, float strength) {
    vec3 p1 = dFdx(position);
    vec3 p2 = dFdy(position);
    vec2 uv1 = dFdx(coords);
    vec2 uv2 = dFdy(coords);
    vec3 p2_perp = cross(p2, normal);
    vec3 p1_perp = cross(normal, p1);
    vec3 t = p2_perp * uv1.x + p1_perp * uv2.x;
    vec3 b = p2_perp * uv1.y + p1_perp * uv2.y;
    float scale = inversesqrt(max(max(dot(t,t),dot(b,b)), 1e-20));
    vec2 xy = (sample_normal.xy * 2.0 - 1.0) * strength * normal_strength;
    return normalize(mat3(t * scale,b * scale,normal) * vec3(xy,sqrt(max(1.0-dot(xy,xy),0.001))));
}

// Rotate a detail normal from the geometric surface onto its authored normal.
// Unlike replacing/mixing the base map, this preserves its brick joints/grain.
vec3 layer_surface_normal(vec3 base, vec3 detail, vec3 geometric) {
    vec3 axis = cross(geometric,base);
    vec3 turn = cross(axis,detail);
    return normalize(detail + turn + cross(axis,turn)/max(1.0+dot(geometric,base),0.0001));
}

void fragment() {
    int pattern = int(round(UV2.x));
    vec3 axis = abs(world_normal);
    vec2 plane = axis.y > 0.5 ? world_position.xz : (axis.x > 0.5 ? world_position.zy : world_position.xy);
    bool detail_wood = pattern == 1 || pattern == 12;
    bool detail_stone = pattern == 2 || pattern == 8 || pattern == 9 || pattern == 14;
    bool detail_bark = pattern == 12 || (pattern == 1 && COLOR.r < 0.5);
    // Repeats per metre. The authored plank fibers run along U, bark along V.
    // Only the seamless granular map is resampled, never the masonry/wood maps.
    vec2 detail_repeat = detail_wood ? (detail_bark ? vec2(1.8,0.10) : vec2(0.10,1.8)) : vec2(0.75);
    vec2 detail_coords = plane * detail_repeat;
    // Evaluate derivatives outside the material branch. Combined pixel-axis
    // footprints cover the densest direction of the stretched wood detail.
    vec2 detail_dx = dFdx(detail_coords);
    vec2 detail_dy = dFdy(detail_coords);
    vec3 position_dx = dFdx(VERTEX);
    vec3 position_dy = dFdy(VERTEX);
    vec2 coords = plane * 0.4;
    vec3 albedo = srgb_to_linear(COLOR.rgb);
    vec3 detail_normal = vec3(0.5,0.5,1.0);
    float relief = 0.65;
    float roughness = 0.86;
    float metal = 0.0;
    float ior = 1.5;
    float roughness_floor = 0.32;
    float organic_patch = 0.0;
    if (organic_detiling && (pattern == 3 || pattern == 4 || pattern == 13)) {
        organic_patch = value_noise(world_position*0.18)*8.0;
    }
    if (pattern == 1 || pattern == 5 || pattern == 6) {
        // Chests and dropped supplies use local face coordinates.
        if (pattern == 5 || pattern == 6) { coords = UV; }
        albedo = texture(wood_color,coords).rgb;
        detail_normal = texture(wood_normal,coords).rgb;
        roughness = texture(wood_rough,coords).r;
        if (pattern == 1 && COLOR.r < 0.5) {
            coords = plane * vec2(0.65,0.4);
            albedo = texture(bark_color,coords).rgb;
            detail_normal = texture(bark_normal,coords).rgb;
            roughness = texture(bark_rough,coords).r;
        }
        if (pattern == 5 || pattern == 6) {
            float band = float(UV.x < 0.055 || UV.x > 0.945 || UV.y < 0.035 || UV.y > 0.965);
            float seam = 1.0 - smoothstep(0.006,0.012,abs(UV.y-0.73));
            float latch = float(pattern == 6 && abs(UV.x-0.5) < 0.045 && UV.y > 0.56 && UV.y < 0.82);
            float iron = max(band,latch);
            albedo = mix(albedo * (1.0-seam*0.65),vec3(0.09,0.10,0.10),iron);
            metal = iron * 0.78;
            roughness = mix(roughness,0.46,iron);
            detail_normal = mix(detail_normal,vec3(0.5,0.5,1.0),iron);
        }
    } else if (pattern == 2) {
        relief = 0.85;
        albedo = texture(stone_color,coords).rgb;
        detail_normal = texture(stone_normal,coords).rgb;
        roughness = texture(stone_rough,coords).r;
    } else if (pattern == 3 || pattern == 4) {
        relief = 0.45;
        coords = plane * 0.65;
        albedo = organic_sample(grass_color,coords,organic_patch);
        detail_normal = organic_sample(grass_normal,coords,organic_patch);
        roughness = organic_sample(grass_rough,coords,organic_patch).r;
        if (COLOR.r > COLOR.g) {
            float soil = dot(albedo,vec3(0.2126,0.7152,0.0722));
            albedo = soil * vec3(0.75,0.53,0.35);
        }
        if (pattern == 4) { albedo *= vec3(0.65,0.8,0.6); }
    } else if (pattern >= 7) {
        // Procedural geological detail preserves distinct surfaces without adding
        // unlicensed assets. Color encodes the block's mineral/vegetation tint.
        vec3 tint = albedo;
        float grain = fract(sin(dot(floor(plane*24.0),vec2(127.1,311.7)))*43758.5453);
        float broad = fract(sin(dot(floor(plane*3.0),vec2(269.5,183.3)))*43758.5453);
        float footprint = max(length(dFdx(plane)),length(dFdy(plane)));
        grain = mix(grain,0.5,smoothstep(0.5,1.5,footprint*24.0));
        broad = mix(broad,0.5,smoothstep(0.5,1.5,footprint*3.0));
        albedo = tint * (0.82+grain*0.3);
        if (pattern == 7) {
            coords = plane * 1.5;
            detail_normal = texture(granular_normal,coords).rgb;
            relief = 0.55;
        } else if (pattern == 8 || pattern == 9 || pattern == 14) {
            relief = 0.8;
            float rock = dot(texture(stone_color,coords).rgb,vec3(0.333));
            albedo = tint * (0.65+rock*1.4);
            detail_normal = texture(stone_normal,coords).rgb;
            roughness = texture(stone_rough,coords).r;
            if (pattern == 9) albedo *= 0.65 + broad*0.7;
            if (pattern == 14) {
                float vein = step(0.67,broad)*step(0.25,grain);
                albedo = mix(vec3(0.18,0.19,0.18)*(0.65+rock),tint,vein);
                metal = vein*0.25;
            }
        } else if (pattern == 10) {
            ior = 1.31;
            coords = plane * 1.2;
            detail_normal = texture(granular_normal,coords).rgb;
            relief = 0.25;
            roughness = 0.92;
            albedo = tint*(0.94+grain*0.06);
        } else if (pattern == 11) {
            ior = 1.31;
            roughness_floor = 0.18;
            coords = plane * 0.5;
            detail_normal = texture(granular_normal,coords).rgb;
            relief = 0.15;
            roughness = 0.22;
            albedo = tint*(0.85+broad*0.15);
        } else if (pattern == 12) {
            coords = plane*vec2(0.65,0.4);
            float bark = dot(texture(bark_color,coords).rgb,vec3(0.333));
            albedo = tint*(0.55+bark*1.8);
            detail_normal = texture(bark_normal,coords).rgb;
            roughness = texture(bark_rough,coords).r;
            if (COLOR.r>0.7) albedo *= 1.0-step(0.87,fract(plane.y*2.0))*step(0.4,broad)*0.72;
        } else if (pattern == 13) {
            relief = 0.45;
            float leaf = dot(organic_sample(grass_color,coords,organic_patch),vec3(0.333));
            albedo = tint*(0.6+leaf*1.8);
            detail_normal = organic_sample(grass_normal,coords,organic_patch);
            roughness = organic_sample(grass_rough,coords,organic_patch).r;
        }
    }
    if (pattern != 0 && pattern != 5 && pattern != 6) {
        float variation = material_noise(world_position*0.65);
        albedo *= 1.0 + material_variation*variation;
        roughness += material_variation*variation*0.3;
    }
    vec3 micro_normal = NORMAL;
    float detail_weight = 0.0;
    if (surface_detail_strength > 0.0 && (detail_wood || detail_stone)) {
        // The existing 512px FastNoiseLite map's base lattice scale is 0.08
        // per texel (~41 grains/tile). Mipmaps filter its finer octaves;
        // fade the addition as base grains approach two pixels in width.
        float grain_footprint = length(abs(detail_dx)+abs(detail_dy)) * 41.0;
        detail_weight = surface_detail_strength * (1.0-smoothstep(0.20,0.50,grain_footprint));
        if (detail_weight > 0.0) {
            vec2 micro = textureGrad(granular_normal,detail_coords,detail_dx,detail_dy).rg * 2.0-1.0;
            roughness += dot(micro,vec2(0.5)) * (detail_wood ? 0.014 : 0.018) * detail_weight;
            vec3 p2_perp = cross(position_dy,NORMAL);
            vec3 p1_perp = cross(NORMAL,position_dx);
            vec3 t = p2_perp * detail_dx.x + p1_perp * detail_dy.x;
            vec3 b = p2_perp * detail_dx.y + p1_perp * detail_dy.y;
            float scale = inversesqrt(max(max(dot(t,t),dot(b,b)),1e-20));
            vec2 slope = micro * (detail_wood ? 0.08 : 0.10) * detail_weight * normal_strength;
            micro_normal = normalize(mat3(t*scale,b*scale,NORMAL) * vec3(slope,sqrt(max(1.0-dot(slope,slope),0.001))));
        }
    }
    ALBEDO = albedo;
    // Smooth bilinear interpolation has zero slope at shared block boundaries;
    // unlike triangle interpolation it remains smooth across the face diagonal.
    vec2 blend = smoothstep(vec2(0.0),vec2(1.0),UV);
    float occluders = mix(mix(corner_ao.x,corner_ao.y,blend.x),
                          mix(corner_ao.w,corner_ao.z,blend.x),blend.y);
    AO = 1.0 - occluders * 0.18 * contact_ao_strength;
    AO_LIGHT_AFFECT = 0.2;
    vec3 mapped_normal = surface_normal(detail_normal,coords,VERTEX,NORMAL,relief);
    if (detail_weight > 0.0 && normal_strength > 0.0) {
        mapped_normal = layer_surface_normal(mapped_normal,micro_normal,NORMAL);
    }
    // Preserve the normal-map relief while rounding only exposed voxel edges.
    mapped_normal = normalize(mapped_normal + bevel_normal(VERTEX,NORMAL,UV) - NORMAL);
    // Increase the specular lobe width for unresolved normal detail.
    float variance = dot(dFdx(mapped_normal),dFdx(mapped_normal)) + dot(dFdy(mapped_normal),dFdy(mapped_normal));
    NORMAL = mapped_normal;
    ROUGHNESS = clamp(sqrt(roughness*roughness + min(variance,0.18)),roughness_floor,0.98);
    METALLIC = metal;
    // Godot's dielectric F0 = 0.16 * SPECULAR^2 (not Blender's legacy 0.08*S).
    SPECULAR = (ior-1.0)/(ior+1.0)/0.4;
}
