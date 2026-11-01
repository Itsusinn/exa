//! Rapier queries are rebuilt from the current local snapshot for each fixed step.
//! No hidden physics state survives a rewind, terrain edit, or session change.
use crate::{DT, HEIGHT, RADIUS};
use protocol::Body;
use rapier3d::{
    control::{CharacterLength, KinematicCharacterController},
    parry::{
        partitioning::{Bvh, BvhBuildStrategy},
        query::DefaultQueryDispatcher,
    },
    prelude::*,
};
use std::collections::BTreeSet;

const SKIN: f32 = 0.001;

pub(super) fn move_character(
    body: &mut Body,
    solid: &impl Fn([i32; 3]) -> bool,
    actors: &[[f32; 3]],
    equipment: &[crate::equipment::CollisionBox],
) {
    let desired = Vector::from(body.velocity) * DT;
    let center = Vector::from(body.position) + Vector::Y * (HEIGHT * 0.5);
    // Keep the existing box footprint: a capsule would alter corner clearance and
    // block-placement occupancy. The extra margin covers contact recovery/nudging.
    let half = Vector::new(RADIUS, HEIGHT * 0.5, RADIUS);
    let reach = half + Vector::splat(desired.length() + 0.5);
    let min = (center - reach).floor().to_array().map(|v| v as i32);
    let max = (center + reach).floor().to_array().map(|v| v as i32);
    let mut colliders = ColliderSet::new();
    let mut voxels = BTreeSet::new();
    // Stable insertion order is shared by authority, prediction and input replay.
    // Query only the swept neighborhood, never rebuild the whole voxel world.
    for y in min[1]..=max[1] {
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                if solid([x, y, z]) {
                    voxels.insert([x, y, z]);
                }
            }
        }
    }
    // Coalesce adjacent solid cells into boxes. Flat floors/walls must not have
    // internal collider edges that can snag a box-shaped character.
    while let Some(&start) = voxels.first() {
        let mut end = start.map(|v| v + 1);
        while voxels.contains(&[end[0], start[1], start[2]]) {
            end[0] += 1;
        }
        while (start[0]..end[0]).all(|x| voxels.contains(&[x, start[1], end[2]])) {
            end[2] += 1;
        }
        while (start[0]..end[0])
            .all(|x| (start[2]..end[2]).all(|z| voxels.contains(&[x, end[1], z])))
        {
            end[1] += 1;
        }
        for x in start[0]..end[0] {
            for y in start[1]..end[1] {
                for z in start[2]..end[2] {
                    voxels.remove(&[x, y, z]);
                }
            }
        }
        let low = Vector::from(start.map(|v| v as f32));
        let high = Vector::from(end.map(|v| v as f32));
        let extents = (high - low) * 0.5;
        colliders.insert(
            ColliderBuilder::cuboid(extents.x, extents.y, extents.z)
                .translation((low + high) * 0.5),
        );
    }
    for shape in equipment {
        let low = Vector::from(shape.min);
        let high = Vector::from(shape.max);
        let extents = (high - low) * 0.5;
        colliders.insert(
            ColliderBuilder::cuboid(extents.x, extents.y, extents.z)
                .translation((low + high) * 0.5),
        );
    }
    let mut nearby: Vec<_> = actors
        .iter()
        .copied()
        .filter(|p| {
            p.iter().all(|v| v.is_finite())
                && (Vector::from(*p) + Vector::Y * (HEIGHT * 0.5) - center)
                    .abs()
                    .cmple(reach + half)
                    .all()
        })
        .collect();
    nearby.sort_by(|a, b| {
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    for position in nearby {
        colliders.insert(
            ColliderBuilder::cuboid(RADIUS, HEIGHT * 0.5, RADIUS)
                .translation(Vector::from(position) + Vector::Y * (HEIGHT * 0.5)),
        );
    }
    // A query-only BVH avoids running a second simulation or contact solver.
    let bvh = Bvh::from_iter(
        BvhBuildStrategy::default(),
        colliders.iter().map(|(handle, collider)| {
            (handle.into_raw_parts().0 as usize, collider.compute_aabb())
        }),
    );
    let bodies = RigidBodySet::new();
    let queries = QueryPipeline {
        dispatcher: &DefaultQueryDispatcher,
        bvh: &bvh,
        bodies: &bodies,
        colliders: &colliders,
        filter: QueryFilter::default(),
    };
    let controller = KinematicCharacterController {
        offset: CharacterLength::Absolute(SKIN),
        normal_nudge_factor: SKIN,
        autostep: None,
        snap_to_ground: None,
        ..Default::default()
    };
    let mut velocity = Vector::from(body.velocity);
    let mut translation = Vector::ZERO;
    body.grounded = false;
    // Separate horizontal steering from gravity, so contact with a floor/wall
    // corner cannot turn vertical motion into unintended horizontal acceleration.
    for delta in [
        Vector::new(desired.x, 0.0, desired.z),
        Vector::Y * desired.y,
    ] {
        let movement = controller.move_shape(
            DT,
            &queries,
            &Cuboid::new(half),
            &Pose::from_translation(center + translation),
            delta,
            |collision| {
                // Obstacle normals are world-space because all colliders are unrotated.
                let normal = collision.hit.normal1;
                let inward = velocity.dot(normal);
                if inward < 0.0 {
                    velocity -= normal * inward;
                }
            },
        );
        translation += movement.translation;
        body.grounded = movement.grounded && desired.y <= 0.0;
    }
    body.position = (Vector::from(body.position) + translation).to_array();
    if body.grounded {
        velocity.y = 0.0;
    }
    body.velocity = velocity.to_array();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{grid::Grid, overlaps, step, step_with_actors, SPEED};
    use protocol::{PlayerInput, AIR, STONE};

    fn body(position: [f32; 3]) -> Body {
        Body {
            position,
            grounded: true,
            ..Body::default()
        }
    }

    #[test]
    fn diagonal_motion_slides_along_wall_across_voxel_seams() {
        let solid = |p: [i32; 3]| p[1] < 0 || p[0] >= 2;
        let mut b = body([1.709, 0.001, 0.5]);
        for _ in 0..120 {
            step(
                &mut b,
                PlayerInput {
                    forward: 1.0,
                    strafe: 1.0,
                    ..Default::default()
                },
                solid,
            );
            assert!(!overlaps(b.position, &solid));
            assert!(b.grounded, "{b:?}");
        }
        assert!((b.position[0] - 1.709).abs() < 0.005, "{b:?}");
        let expected_z = 0.5 - SPEED / 2.0f32.sqrt() * DT * 120.0;
        assert!((b.position[2] - expected_z).abs() < 0.03, "{b:?}");
        assert!(b.velocity[0].abs() < 0.001);
    }

    #[test]
    fn ceiling_cancels_upward_velocity_and_airborne_jump_does_not_reset_it() {
        let mut b = body([0.5, 0.001, 0.5]);
        step(
            &mut b,
            PlayerInput {
                jump: true,
                ..Default::default()
            },
            |p| p[1] < 0 || p[1] >= 2,
        );
        step(&mut b, PlayerInput::default(), |p| p[1] < 0 || p[1] >= 2);
        assert!(b.position[1] < 0.251);
        assert!(b.velocity[1].abs() < 0.001, "{b:?}");
        assert!(!b.grounded);
        step(
            &mut b,
            PlayerInput {
                jump: true,
                ..Default::default()
            },
            |p| p[1] < 0,
        );
        assert!(b.velocity[1] < 0.0);
    }

    #[test]
    fn falling_character_does_not_tunnel_through_floor_or_actor() {
        for actor in [false, true] {
            let floor = if actor { HEIGHT } else { 1.0 };
            let mut b = body([0.5, floor + 0.2, 0.5]);
            b.grounded = false;
            b.velocity[1] = -20.0;
            let actors = if actor { vec![[0.5, 0.0, 0.5]] } else { vec![] };
            for _ in 0..4 {
                step_with_actors(
                    &mut b,
                    PlayerInput::default(),
                    |p| !actor && p[1] == 0,
                    &actors,
                );
            }
            assert!(b.grounded && b.velocity[1] == 0.0, "{b:?}");
            assert!((b.position[1] - floor).abs() < 0.005, "{b:?}");
        }
    }

    #[test]
    fn edits_immediately_change_support_and_obstacles() {
        let mut map = Grid::empty();
        map.set([5, 2, 5], STONE);
        let mut b = body([5.5, 3.001, 5.5]);
        step(&mut b, PlayerInput::default(), |p| map.solid(p));
        assert!(b.grounded);
        map.set([5, 2, 5], AIR);
        for _ in 0..10 {
            step(&mut b, PlayerInput::default(), |p| map.solid(p));
        }
        assert!(b.position[1] < 2.0 && !b.grounded);

        b = body([5.5, 0.001, 5.5]);
        map.set([6, 0, 5], STONE);
        let input = PlayerInput {
            strafe: 1.0,
            ..Default::default()
        };
        for _ in 0..10 {
            step(&mut b, input, |p| map.solid(p));
        }
        assert!(b.position[0] < 5.711);
        // A one-block obstacle must still require jumping (no auto-step).
        assert!(b.position[1] < 0.01);
        map.set([6, 0, 5], AIR);
        for _ in 0..10 {
            step(&mut b, input, |p| map.solid(p));
        }
        assert!(b.position[0] > 6.5);
    }

    #[test]
    fn unknown_chunk_boundary_blocks_until_current_chunk_arrives() {
        let mut map = Grid::unknown();
        assert!(map.apply_chunk([0, 0, 0], &vec![AIR; 4096]));
        let mut b = body([15.5, 0.001, 5.5]);
        let input = PlayerInput {
            strafe: 1.0,
            ..Default::default()
        };
        for _ in 0..10 {
            step(&mut b, input, |p| map.solid(p));
        }
        assert!(b.position[0] < 15.711 && !overlaps(b.position, &|p| map.solid(p)));
        assert!(map.apply_chunk([16, 0, 0], &vec![AIR; 4096]));
        for _ in 0..10 {
            step(&mut b, input, |p| map.solid(p));
        }
        assert!(b.position[0] > 16.5);
    }

    #[test]
    fn snapshot_replay_is_exact_and_independent_of_actor_order() {
        let solid = |p: [i32; 3]| p[1] < 0 || p[0] >= 5;
        let actors = [[2.0, 0.0, 0.5], [2.0, 0.0, -0.1]];
        let input = PlayerInput {
            strafe: 1.0,
            ..Default::default()
        };
        let mut authority = body([0.5, 0.001, 0.5]);
        for _ in 0..4 {
            step_with_actors(&mut authority, input, solid, &actors);
        }
        let snapshot = authority;
        for _ in 0..30 {
            step_with_actors(&mut authority, input, solid, &actors);
        }
        let mut replay = snapshot;
        for _ in 0..30 {
            step_with_actors(&mut replay, input, solid, &[actors[1], actors[0]]);
        }
        assert_eq!(authority, replay);
        assert!(authority.position[0] < 1.421);
    }
}
