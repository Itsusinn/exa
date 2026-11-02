//! Pure CPU geometry shared by the Godot presentation adapter.
#[derive(Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub uv: [f32; 2],
    pub pattern: u32,
    pub occlusion: f32,
    /// Four two-bit corner occluder counts, shared by both triangles (UV order).
    pub face_occlusion: u8,
}
#[derive(Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
}
impl Mesh {
    pub fn face(&mut self, points: [[f32; 3]; 4], normal: [f32; 3], color: [f32; 3], pattern: u32) {
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for i in [0, 2, 1, 0, 3, 2] {
            self.vertices.push(Vertex {
                position: points[i],
                normal,
                color,
                uv: uv[i],
                pattern,
                occlusion: 1.0,
                face_occlusion: 0,
            });
        }
    }
    pub fn cuboid(&mut self, min: [f32; 3], size: [f32; 3], color: [f32; 3], pattern: u32) {
        for face in 0..6 {
            self.block_face(min, size, face, color, pattern);
        }
    }
    pub fn block_face(
        &mut self,
        min: [f32; 3],
        size: [f32; 3],
        face: usize,
        color: [f32; 3],
        pattern: u32,
    ) {
        const POINTS: [[[f32; 3]; 4]; 6] = [
            [[0., 0., 1.], [0., 0., 0.], [0., 1., 0.], [0., 1., 1.]],
            [[1., 0., 0.], [1., 0., 1.], [1., 1., 1.], [1., 1., 0.]],
            [[0., 0., 0.], [0., 0., 1.], [1., 0., 1.], [1., 0., 0.]],
            [[0., 1., 1.], [0., 1., 0.], [1., 1., 0.], [1., 1., 1.]],
            [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
            [[1., 0., 1.], [0., 0., 1.], [0., 1., 1.], [1., 1., 1.]],
        ];
        let points = POINTS[face].map(|p| std::array::from_fn(|i| min[i] + p[i] * size[i]));
        self.face(points, NORMALS[face], color, pattern);
    }
}
pub const NORMALS: [[f32; 3]; 6] = [
    [-1., 0., 0.],
    [1., 0., 0.],
    [0., -1., 0.],
    [0., 1., 0.],
    [0., 0., -1.],
    [0., 0., 1.],
];
