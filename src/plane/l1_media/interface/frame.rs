//! Media-owned encoded stereo frame passed from codec/source preparation into the bridge.

use crate::plane::l1_media::clock::frame::FrameId;

pub struct EncodedStereoFrame<P> {
    pub frame_id: FrameId,
    pub left: P,
    pub right: P,
}
