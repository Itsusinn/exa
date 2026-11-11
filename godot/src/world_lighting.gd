extends RefCounted
## Static daylight calibration. Apply only at scene setup or explicit review
## switches; never update the sky/radiance resource from the frame loop.
## ACES and exposure stay fixed so the before/after is a lighting comparison.

static func apply(environment: Environment, sun: DirectionalLight3D, enhanced: bool = true) -> void:
	# Preserve the same sun direction and cascaded-shadow coverage in both profiles.
	sun.rotation_degrees = Vector3(-38,-35,0)
	sun.light_color = Color(1.0,0.975,0.92)
	sun.light_energy = 1.10 if enhanced else 1.0
	sun.light_angular_distance = 0.8 if enhanced else 1.2
	sun.shadow_enabled = true
	sun.directional_shadow_max_distance = 100
	sun.directional_shadow_mode = DirectionalLight3D.SHADOW_PARALLEL_4_SPLITS
	sun.directional_shadow_blend_splits = true
	sun.directional_shadow_split_1 = 0.1
	sun.directional_shadow_split_2 = 0.25
	sun.directional_shadow_split_3 = 0.5
	sun.shadow_bias = 0.03
	sun.shadow_normal_bias = 0.5

	var horizon := Color("bfcbd1")
	var sky_material := ProceduralSkyMaterial.new()
	sky_material.sky_top_color = Color("6f92b5") if enhanced else Color("7896b3")
	sky_material.sky_horizon_color = horizon
	sky_material.ground_horizon_color = horizon
	sky_material.ground_bottom_color = Color("3f443d") if enhanced else Color("424b39")
	sky_material.sky_curve = 0.25 if enhanced else 0.15
	sky_material.ground_curve = 0.02
	sky_material.sky_energy_multiplier = 1.0
	sky_material.ground_energy_multiplier = 1.0
	sky_material.sun_angle_max = 8.0 if enhanced else 30.0
	sky_material.sun_curve = 0.15
	var sky := Sky.new()
	sky.process_mode = Sky.PROCESS_MODE_QUALITY if enhanced else Sky.PROCESS_MODE_AUTOMATIC
	sky.radiance_size = Sky.RADIANCE_SIZE_256
	sky.sky_material = sky_material
	environment.background_mode = Environment.BG_SKY
	environment.background_energy_multiplier = 1.0
	environment.sky = sky
	environment.ambient_light_source = Environment.AMBIENT_SOURCE_SKY
	environment.ambient_light_sky_contribution = 1.0
	# At sky contribution=1, ambient_light_energy does not scale sky radiance.
	# Keep the legacy value; calibrate the sky material rather than fake fill light.
	environment.ambient_light_energy = 0.3
	environment.reflected_light_source = Environment.REFLECTION_SOURCE_SKY
	environment.tonemap_mode = Environment.TONE_MAPPER_ACES
	environment.tonemap_exposure = 1.0
	environment.tonemap_white = 1.0

	# sdfgi_enabled belongs to the player's existing GI setting; do not change it.
	environment.sdfgi_min_cell_size = 0.35
	environment.sdfgi_cascades = 4
	environment.sdfgi_use_occlusion = true
	environment.sdfgi_read_sky_light = true
	environment.sdfgi_energy = 1.0
	environment.sdfgi_bounce_feedback = 0.40 if enhanced else 0.30
	environment.ssao_enabled = true
	environment.ssao_radius = 0.45
	# Voxel contact AO already supplies static creases; avoid double-dark contacts.
	environment.ssao_intensity = 0.50 if enhanced else 0.65
	environment.ssao_light_affect = 0.0
	environment.ssao_ao_channel_affect = 0.0

	# Screen-space rays see only visible opaque geometry. With GI enabled, sky and
	# SDFGI supply fallback reflections. With GI disabled, only the sky remains.
	# Keep the engine's half-resolution SSR budget; no per-device reflection probes.
	environment.ssr_enabled = enhanced
	environment.ssr_max_steps = 48 if enhanced else 64
	environment.ssr_depth_tolerance = 0.2 if enhanced else 0.5
	environment.ssr_fade_in = 0.15
	environment.ssr_fade_out = 2.0
	apply_atmosphere(environment, enhanced)
	if not enhanced:
		# Keep the historical phase-1 lighting reference, including its sky haze.
		environment.fog_sky_affect = 1.0

## Artistic distance layering, not a physical atmospheric scattering integral.
## Independent of daylight/GI/material profiles; false restores phase-3A fog.
static func apply_atmosphere(environment: Environment, enhanced: bool = true) -> void:
	environment.fog_enabled = true
	environment.fog_light_color = Color("bfcbd1")
	environment.fog_light_energy = 1.0
	environment.fog_sun_scatter = 0.0
	environment.fog_height = 0.0
	environment.fog_height_density = 0.0
	environment.fog_aerial_perspective = 0.0
	# Keep sky blending unchanged: standard sky fog is independent of depth density.
	environment.fog_sky_affect = 0.25
	environment.fog_mode = Environment.FOG_MODE_DEPTH if enhanced else Environment.FOG_MODE_EXPONENTIAL
	# In depth mode density is maximum opacity, NOT the exponential per-metre rate.
	# Godot uses pow(smoothstep(begin, end, radial_distance), curve) * density.
	# 16 m stays clear, 32 m reaches ~2.5%, 48 m ~13.6%, and 64 m caps at 22%.
	# This fits the existing 7x7 streamed columns (48-64 m cardinal, farther on
	# diagonals); it intentionally cannot hide all edges of the resident terrain.
	environment.fog_density = 0.22 if enhanced else 0.0015
	environment.fog_depth_begin = 16.0 if enhanced else 10.0
	environment.fog_depth_end = 64.0 if enhanced else 100.0
	environment.fog_depth_curve = 1.6 if enhanced else 1.0
	# No height or volume fog: short indoor views remain clear, but distant indoor
	# surfaces still receive distance fog because this effect has no room occlusion.

## The hierarchy covers the 2048m camera range. Keep local views clear and blend
## only toward the outer horizon. Loading gaps remain explicit pending coverage.
static func apply_distance_horizon(environment: Environment) -> void:
	apply_atmosphere(environment,true)
	environment.fog_depth_begin = 320.0
	environment.fog_depth_end = 1950.0
	environment.fog_depth_curve = 2.0
	environment.fog_density = 1.0
	# Match the view-dependent sky radiance instead of leaving a bright silhouette
	# against the darker lower hemisphere when distant geometry reaches full fog.
	environment.fog_aerial_perspective = 1.0
