//! Shared voxel targeting for authoritative edits and crosshair feedback.
use crate::grid::Grid;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHit {
    pub position: [i32; 3],
    pub normal: [i32; 3],
}

pub fn target_block(map: &Grid, feet: [f32; 3], yaw: f32, pitch: f32) -> Option<BlockHit> {
    let hit = crate::equipment::target(map, feet, yaw, pitch)?;
    hit.equipment.is_none().then_some(BlockHit {
        position: hit.position,
        normal: hit.normal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::STONE;
    #[test]
    fn first_hit_face_range_unknown_and_invalid_angles() {
        let mut map = Grid::empty();
        map.set([10, 2, 7], STONE);
        let feet = [10.5, 1.0, 10.5];
        assert_eq!(
            target_block(&map, feet, 0.0, 0.0),
            Some(BlockHit {
                position: [10, 2, 7],
                normal: [0, 0, 1],
            })
        );
        map.set([10, 2, 9], STONE);
        assert_eq!(
            target_block(&map, feet, 0.0, 0.0).unwrap().position,
            [10, 2, 9]
        );
        map.set([10, 2, 9], 255);
        assert!(target_block(&map, feet, 0.0, 0.0).is_none());
        assert!(target_block(&map, feet, f32::NAN, 0.0).is_none());
        assert!(target_block(&map, feet, 0.0, f32::INFINITY).is_none());
        assert!(target_block(&map, [10.5, 1.0, 15.0], 0.0, 0.0).is_none());
        assert!(target_block(&map, [-1.0, 1.0, 10.0], 0.0, 0.0).is_none());
    }
}
