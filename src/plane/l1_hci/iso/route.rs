use std::collections::HashMap;

use crate::{
    architecture::types::{SessionId, StreamId},
    plane::l1_control::interface::stream::AudioSide,
};

use super::cig_cis::CisRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamRouteKey {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub side: AudioSide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StereoStreamRouteBinding {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub left: CisRef,
    pub right: CisRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRouteConfigError {
    DuplicateCis(CisRef),
    CigMismatch {
        left_cig_id: u8,
        right_cig_id: u8,
    },
    ConflictingBinding {
        session_id: SessionId,
        stream_id: StreamId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRouteFailureKind {
    Config(StreamRouteConfigError),
    CisNotReady(CisRef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamRouteFailure {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub kind: StreamRouteFailureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRouteState {
    Bound {
        session_id: SessionId,
        stream_id: StreamId,
    },
    Unbound {
        session_id: SessionId,
        stream_id: StreamId,
        was_bound: bool,
    },
    Failed(StreamRouteFailure),
}

#[derive(Default)]
pub struct StreamRouteTable {
    routes: HashMap<StreamRouteKey, CisRef>,
}

impl StereoStreamRouteBinding {
    pub fn validate(&self) -> Result<(), StreamRouteConfigError> {
        if self.left == self.right {
            return Err(StreamRouteConfigError::DuplicateCis(
                self.left,
            ));
        }
        if self.left.cig_id != self.right.cig_id {
            return Err(StreamRouteConfigError::CigMismatch {
                left_cig_id: self.left.cig_id,
                right_cig_id: self.right.cig_id,
            });
        }
        Ok(())
    }

    fn key(&self, side: AudioSide) -> StreamRouteKey {
        StreamRouteKey {
            session_id: self.session_id,
            stream_id: self.stream_id,
            side,
        }
    }
}

impl StreamRouteTable {
    pub fn bind(
        &mut self,
        binding: StereoStreamRouteBinding,
    ) -> Result<(), StreamRouteConfigError> {
        binding.validate()?;

        let left_key = binding.key(AudioSide::Left);
        let right_key = binding.key(AudioSide::Right);
        let existing_left = self.routes.get(&left_key).copied();
        let existing_right = self.routes.get(&right_key).copied();

        match (existing_left, existing_right) {
            (None, None) => {
                self.routes.insert(left_key, binding.left);
                self.routes.insert(right_key, binding.right);
                Ok(())
            }
            (Some(left), Some(right))
                if left == binding.left && right == binding.right =>
            {
                // Reliable control requests may be replayed. Exact replay is
                // idempotent; a different mapping is never accepted.
                Ok(())
            }
            _ => Err(StreamRouteConfigError::ConflictingBinding {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
            }),
        }
    }

    pub fn resolve(
        &self,
        session_id: SessionId,
        stream_id: StreamId,
        side: AudioSide,
    ) -> Option<CisRef> {
        self.routes
            .get(&StreamRouteKey {
                session_id,
                stream_id,
                side,
            })
            .copied()
    }

    pub fn unbind(
        &mut self,
        session_id: SessionId,
        stream_id: StreamId,
    ) -> bool {
        let left = self.routes.remove(&StreamRouteKey {
            session_id,
            stream_id,
            side: AudioSide::Left,
        });
        let right = self.routes.remove(&StreamRouteKey {
            session_id,
            stream_id,
            side: AudioSide::Right,
        });
        left.is_some() || right.is_some()
    }

    pub fn remove_cis(&mut self, cis: CisRef) -> usize {
        let before = self.routes.len();
        self.routes.retain(|_, mapped| *mapped != cis);
        before - self.routes.len()
    }

    pub fn contains_stream(
        &self,
        session_id: SessionId,
        stream_id: StreamId,
    ) -> bool {
        self.resolve(session_id, stream_id, AudioSide::Left)
            .is_some()
            && self
                .resolve(
                    session_id,
                    stream_id,
                    AudioSide::Right,
                )
                .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> StereoStreamRouteBinding {
        StereoStreamRouteBinding {
            session_id: SessionId(7),
            stream_id: StreamId(11),
            left: CisRef {
                cig_id: 1,
                cis_id: 1,
            },
            right: CisRef {
                cig_id: 1,
                cis_id: 2,
            },
        }
    }

    #[test]
    fn stereo_binding_resolves_by_session_stream_and_side() {
        let mut routes = StreamRouteTable::default();
        routes.bind(binding()).unwrap();

        assert_eq!(
            routes.resolve(
                SessionId(7),
                StreamId(11),
                AudioSide::Left,
            ),
            Some(CisRef {
                cig_id: 1,
                cis_id: 1,
            })
        );
        assert_eq!(
            routes.resolve(
                SessionId(7),
                StreamId(11),
                AudioSide::Right,
            ),
            Some(CisRef {
                cig_id: 1,
                cis_id: 2,
            })
        );
        assert_eq!(
            routes.resolve(
                SessionId(8),
                StreamId(11),
                AudioSide::Left,
            ),
            None
        );
    }

    #[test]
    fn exact_replay_is_idempotent_but_conflicting_rebind_is_rejected() {
        let mut routes = StreamRouteTable::default();
        let first = binding();
        routes.bind(first).unwrap();
        routes.bind(first).unwrap();

        let mut conflicting = first;
        conflicting.right = CisRef {
            cig_id: 1,
            cis_id: 3,
        };
        assert_eq!(
            routes.bind(conflicting),
            Err(StreamRouteConfigError::ConflictingBinding {
                session_id: SessionId(7),
                stream_id: StreamId(11),
            })
        );
        assert_eq!(
            routes.resolve(
                SessionId(7),
                StreamId(11),
                AudioSide::Right,
            ),
            Some(first.right)
        );
    }

    #[test]
    fn invalid_stereo_route_is_rejected_before_mutation() {
        let mut routes = StreamRouteTable::default();
        let mut duplicate = binding();
        duplicate.right = duplicate.left;
        assert_eq!(
            routes.bind(duplicate),
            Err(StreamRouteConfigError::DuplicateCis(
                duplicate.left,
            ))
        );

        let mut split_cig = binding();
        split_cig.right.cig_id = 2;
        assert_eq!(
            routes.bind(split_cig),
            Err(StreamRouteConfigError::CigMismatch {
                left_cig_id: 1,
                right_cig_id: 2,
            })
        );
        assert!(!routes.contains_stream(SessionId(7), StreamId(11)));
    }

    #[test]
    fn teardown_can_remove_whole_stream_or_any_route_using_a_cis() {
        let mut routes = StreamRouteTable::default();
        let value = binding();
        routes.bind(value).unwrap();

        assert_eq!(routes.remove_cis(value.right), 1);
        assert!(!routes.contains_stream(value.session_id, value.stream_id));
        assert_eq!(
            routes.resolve(
                value.session_id,
                value.stream_id,
                AudioSide::Left,
            ),
            Some(value.left)
        );

        assert!(routes.unbind(value.session_id, value.stream_id));
        assert_eq!(
            routes.resolve(
                value.session_id,
                value.stream_id,
                AudioSide::Left,
            ),
            None
        );
    }
}
