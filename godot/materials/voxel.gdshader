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
varying vec3 world_position;
varying vec3 world_normal;
varying vec4 corner_ao;
uniform float contact_ao_strength : hint_range(0.0,1.0) = 0.85;

void vertex() {
    world_position = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
    world_normal = normalize(MODEL_NORMAL_MATRIX * NORMAL);
    // Decode before interpolation. Each face carries the same four samples on
    // both triangles, so the diagonal cannot introduce a lighting crease.
    int packed_ao = int(round(UV2.y));
    corner_ao = vec4(float(packed_ao & 3),float((packed_ao >> 2) & 3),
                     float((packed_ao >> 4) & 3),float((packed_ao >> 6) & 3));
}

// Derive a tangent frame from the actual surface projection. Chunk meshes do
// not carry tangents; this also keeps rotated actor/item meshes consistent.
vec3 surface_normal(vec3 sample_normal, vec2 coords, vec3 position, vec3 normal) {
    vec3 p1 = dFdx(position);
    vec3 p2 = dFdy(position);
    vec2 uv1 = dFdx(coords);
    vec2 uv2 = dFdy(coords);
    vec3 p2_perp = cross(p2, normal);
    vec3 p1_perp = cross(normal, p1);
    vec3 t = p2_perp * uv1.x + p1_perp * uv2.x;
    vec3 b = p2_perp * uv1.y + p1_perp * uv2.y;
    float scale = inversesqrt(max(max(dot(t,t),dot(b,b)), 0.000001));
    vec2 xy = (sample_normal.xy * 2.0 - 1.0) * 0.65;
    return normalize(mat3(t * scale,b * scale,normal) * vec3(xy,sqrt(max(1.0-dot(xy,xy),0.001))));
}

void fragment() {
    int pattern = int(round(UV2.x));
    vec3 axis = abs(world_normal);
    vec2 plane = axis.y > 0.5 ? world_position.xz : (axis.x > 0.5 ? world_position.zy : world_position.xy);
    vec2 coords = plane * 0.4;
    vec3 albedo = pow(COLOR.rgb,vec3(2.2));
    vec3 detail_normal = vec3(0.5,0.5,1.0);
    float roughness = 0.86;
    float metal = 0.0;
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
        albedo = texture(stone_color,coords).rgb;
        detail_normal = texture(stone_normal,coords).rgb;
        roughness = texture(stone_rough,coords).r;
    } else if (pattern == 3 || pattern == 4) {
        coords = plane * 0.65;
        albedo = texture(grass_color,coords).rgb;
        detail_normal = texture(grass_normal,coords).rgb;
        roughness = texture(grass_rough,coords).r;
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
        if (pattern == 8 || pattern == 9 || pattern == 14) {
            float rock = dot(texture(stone_color,coords).rgb,vec3(0.333));
            albedo = tint * (0.65+rock*1.4);
            detail_normal = texture(stone_normal,coords).rgb;
            if (pattern == 9) albedo *= 0.65 + broad*0.7;
            if (pattern == 14) {
                float vein = step(0.67,broad)*step(0.25,grain);
                albedo = mix(vec3(0.18,0.19,0.18)*(0.65+rock),tint,vein);
                metal = vein*0.25;
            }
        } else if (pattern == 10) {
            roughness = 0.92;
            albedo = tint*(0.94+grain*0.06);
        } else if (pattern == 11) {
            roughness = 0.32;
            albedo = tint*(0.85+broad*0.15);
        } else if (pattern == 12) {
            coords = plane*vec2(0.65,0.4);
            float bark = dot(texture(bark_color,coords).rgb,vec3(0.333));
            albedo = tint*(0.55+bark*1.8);
            detail_normal = texture(bark_normal,coords).rgb;
            if (COLOR.r>0.7) albedo *= 1.0-step(0.87,fract(plane.y*2.0))*step(0.4,broad)*0.72;
        } else if (pattern == 13) {
            float leaf = dot(texture(grass_color,coords).rgb,vec3(0.333));
            albedo = tint*(0.6+leaf*1.8);
            detail_normal = texture(grass_normal,coords).rgb;
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
    vec3 mapped_normal = surface_normal(detail_normal,coords,VERTEX,NORMAL);
    // Increase the specular lobe width for unresolved normal detail.
    float variance = dot(dFdx(mapped_normal),dFdx(mapped_normal)) + dot(dFdy(mapped_normal),dFdy(mapped_normal));
    NORMAL = mapped_normal;
    ROUGHNESS = clamp(sqrt(roughness*roughness + min(variance,0.18)),0.32,0.98);
    METALLIC = metal;
}
