# Material sources

Downloaded 2026-10-27 from Poly Haven. These original texture assets are
licensed under **CC0 1.0 Universal** and may be redistributed with this project.

- [Weathered Brown Planks](https://polyhaven.com/a/weathered_brown_planks)
- [Stone Brick Wall 001](https://polyhaven.com/a/stone_brick_wall_001)
- [Leafy Grass](https://polyhaven.com/a/leafy_grass)
- [Bark Brown 02](https://polyhaven.com/a/bark_brown_02)

[Poly Haven asset license](https://polyhaven.com/license) ·
[CC0 legal text](https://creativecommons.org/publicdomain/zero/1.0/legalcode)

The JPG files are the unmodified 1K diffuse, OpenGL normal and roughness maps.
`sources.json` records the exact download URLs, upstream MD5 and local SHA-256.
No preview renders, website branding or example scenes are included.

`../granular_normal.tres` is a project-authored procedural normal texture
definition (FastNoiseLite seed 731), not a downloaded Poly Haven asset.
It generates a seamless 512px normal map for sand/clay, snow and ice.

`../frontier_background.png` is a screenshot rendered by this project's own
Godot scene using these materials, captured without HUD via `tests/visuals.gd`.
