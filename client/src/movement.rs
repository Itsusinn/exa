use crate::session::Session;
use exa_protocol::PlayerInput;
use exa_simulation::DT;

/// Render one simulation step behind, interpolating only completed predictions.
/// Mouse look is deliberately independent so turning stays immediate.
#[derive(Default)]
pub struct Movement {
    accumulator: f32,
    generation: Option<u64>,
    previous: [f32; 3],
    current: [f32; 3],
}

impl Movement {
    pub fn sync(&mut self, session: &Session) {
        let position = session.model.body.position;
        if !session.model.ready || self.generation != Some(session.generation) {
            self.accumulator = 0.0;
            self.generation = session.model.ready.then_some(session.generation);
            self.previous = position;
            self.current = position;
        } else if self.current != position {
            // Authority corrected prediction (including respawn). Do not blend
            // from an invalid old location or interpolate across a teleport.
            self.previous = position;
            self.current = position;
        }
    }

    pub fn advance(
        &mut self,
        session: &mut Session,
        elapsed: f32,
        mut input: impl FnMut() -> PlayerInput,
    ) {
        self.sync(session);
        if !session.model.ready {
            return;
        }
        self.accumulator = (self.accumulator + elapsed.clamp(0.0, 0.1)).min(0.1);
        while self.accumulator >= DT {
            self.accumulator -= DT;
            self.previous = self.current;
            session.input(input());
            // A rejected send leaves the model unchanged: never extrapolate.
            self.current = session.model.body.position;
        }
    }

    pub fn position(&self) -> [f32; 3] {
        let alpha = (self.accumulator / DT).clamp(0.0, 1.0);
        std::array::from_fn(|i| self.previous[i] + (self.current[i] - self.previous[i]) * alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_client::{NetworkEvent, Transport};
    use exa_protocol::{ClientMessage, ServerMessage, Snapshot3d};
    use exa_simulation::{distance, overlaps, SPEED};

    struct Queue {
        full: bool,
    }
    impl Transport for Queue {
        fn send(&self, _: ClientMessage) -> Result<(), String> {
            if self.full {
                Err("full".into())
            } else {
                Ok(())
            }
        }
        fn poll(&mut self) -> Option<NetworkEvent> {
            None
        }
    }

    fn forward() -> PlayerInput {
        PlayerInput {
            forward: 1.0,
            ..Default::default()
        }
    }

    #[test]
    fn camera_moves_every_frame_at_60_144_hz_and_irregular_frame_times() {
        for frames in [
            vec![1.0 / 60.0],
            vec![1.0 / 144.0],
            vec![0.007, 0.013, 0.021, 0.045],
        ] {
            let mut session = Session::default();
            session.load_demo();
            let mut movement = Movement::default();
            movement.advance(&mut session, DT * 2.0, forward);
            let mut previous = movement.position();
            for elapsed in frames.iter().cycle().take(60) {
                movement.advance(&mut session, *elapsed, forward);
                let position = movement.position();
                let displacement = previous[2] - position[2];
                assert!(
                    (displacement - SPEED * elapsed).abs() < 0.0001,
                    "elapsed={elapsed}, displacement={displacement}"
                );
                previous = position;
            }
        }
    }

    #[test]
    fn stopping_settles_within_one_tick_and_never_extrapolates() {
        let mut session = Session::default();
        session.load_demo();
        let mut movement = Movement::default();
        movement.advance(&mut session, DT * 1.5, forward);
        let stopped = session.model.body.position;
        assert!(movement.position()[2] > stopped[2]);
        for _ in 0..12 {
            movement.advance(&mut session, DT / 4.0, PlayerInput::default);
            assert!(movement.position()[2] >= stopped[2]);
        }
        assert_eq!(movement.position(), session.model.body.position);
    }

    #[test]
    fn jumping_and_wall_collision_use_completed_physics_steps() {
        let mut session = Session::default();
        session.load_demo();
        let mut movement = Movement::default();
        let start = session.model.body.position;
        let mut peak = start[1];
        for frame in 0..600 {
            movement.advance(&mut session, DT / 4.0, || PlayerInput {
                jump: frame < 4,
                ..forward()
            });
            let position = movement.position();
            peak = peak.max(position[1]);
            assert!(!overlaps(position, &|p| session.model.terrain.solid(p)));
            assert!(distance(position, session.model.body.position) < 0.7);
        }
        assert!(peak > start[1] + 1.0);
        assert!(session.model.body.grounded);
        assert!(session.model.body.position[2] > 19.0);
        assert!(session.model.body.position[2] < 19.4);
    }

    #[test]
    fn correction_disconnect_and_new_session_discard_old_positions() {
        let mut session = Session::default();
        session.load_demo();
        let mut movement = Movement::default();
        movement.advance(&mut session, DT * 1.5, forward);
        session.model.body.position = [40.0, 7.0, 40.0];
        movement.sync(&session);
        assert_eq!(movement.position(), session.model.body.position);
        session.disconnect();
        movement.advance(&mut session, 0.1, forward);
        assert_eq!(movement.position(), session.model.body.position);
        session.load_demo();
        movement.sync(&session);
        assert_eq!(movement.position(), session.model.body.position);
        assert_eq!(movement.accumulator, 0.0);
    }

    #[test]
    fn full_send_queue_does_not_render_unaccepted_movement() {
        let mut session = Session::default();
        session.load_demo();
        session.demo = false;
        session.transport = Some(Box::new(Queue { full: true }));
        let start = session.model.body;
        let mut movement = Movement::default();
        for _ in 0..30 {
            movement.advance(&mut session, DT / 2.0, forward);
            assert_eq!(session.model.body, start);
            assert_eq!(movement.position(), start.position);
            assert!(session.model.pending.is_empty());
        }
    }

    #[test]
    fn matching_ack_preserves_interpolation_between_prediction_steps() {
        let mut session = Session::default();
        session.load_demo();
        session.demo = false;
        session.transport = Some(Box::new(Queue { full: false }));
        let mut movement = Movement::default();
        movement.advance(&mut session, DT, forward);
        let acknowledged = session.model.body;
        movement.advance(&mut session, DT, forward);
        movement.advance(&mut session, DT / 2.0, forward);
        let displayed = movement.position();
        let predicted = session.model.body;
        session.model.apply(ServerMessage::World3d(Snapshot3d {
            flying: false,
            container: None,
            container_error: String::new(),
            seq: 1,
            body: acknowledged,
            actors: session.model.actors.clone(),
            items: vec![],
            hp: 100,
            inventory: exa_protocol::PlayerInventory::default(),
            log: vec![],
        }));
        movement.advance(&mut session, 0.0, forward);
        assert_eq!(session.model.pending.len(), 1);
        assert_eq!(session.model.body, predicted);
        assert_eq!(movement.position(), displayed);
        movement.advance(&mut session, DT / 4.0, forward);
        assert!((displayed[2] - movement.position()[2] - SPEED * DT / 4.0).abs() < 0.0001);
    }
}
