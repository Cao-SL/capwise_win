use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::AvatarWorker;
#[cfg(any(target_os = "macos", test))]
mod paint;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub enum AvatarKind {
    #[default]
    Camera = 0,
    Cat = 1,
    Bear = 2,
    Fox = 3,
}

impl AvatarKind {
    pub const ALL: [Self; 4] = [Self::Camera, Self::Cat, Self::Bear, Self::Fox];
}

#[derive(Debug, Default)]
pub struct AvatarControl(AtomicU64);

impl AvatarControl {
    pub fn snapshot(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    pub fn kind(&self) -> AvatarKind {
        Self::kind_at(self.snapshot())
    }

    pub fn kind_at(snapshot: u64) -> AvatarKind {
        AvatarKind::ALL
            .get((snapshot & 255) as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn set(&self, kind: AvatarKind) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                Some((old & !255).wrapping_add(256) | kind as u64)
            });
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub left_eye: f32,
    pub right_eye: f32,
    pub mouth: f32,
    pub smile: f32,
}

#[cfg(any(target_os = "macos", test))]
impl Default for Pose {
    fn default() -> Self {
        Self {
            yaw: 0.,
            pitch: 0.,
            roll: 0.,
            left_eye: 1.,
            right_eye: 1.,
            mouth: 0.,
            smile: 0.,
        }
    }
}

#[cfg(any(target_os = "macos", test))]
impl Pose {
    pub fn approach(&mut self, next: Self, amount: f32) {
        fn blend(old: &mut f32, next: f32, amount: f32) {
            if next.is_finite() {
                *old += (next - *old) * amount;
            }
        }
        blend(&mut self.yaw, next.yaw.clamp(-1., 1.), amount);
        blend(&mut self.pitch, next.pitch.clamp(-1., 1.), amount);
        blend(&mut self.roll, next.roll.clamp(-0.5, 0.5), amount);
        blend(
            &mut self.left_eye,
            next.left_eye.clamp(0., 1.),
            amount.max(0.65),
        );
        blend(
            &mut self.right_eye,
            next.right_eye.clamp(0., 1.),
            amount.max(0.65),
        );
        blend(&mut self.mouth, next.mouth.clamp(0., 1.), amount.max(0.65));
        blend(&mut self.smile, next.smile.clamp(0., 1.), amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_back_invalidates_in_flight_frames() {
        let control = AvatarControl::default();
        control.set(AvatarKind::Cat);
        let first = control.snapshot();
        control.set(AvatarKind::Bear);
        control.set(AvatarKind::Cat);
        assert_eq!(control.kind(), AvatarKind::Cat);
        assert_ne!(first, control.snapshot());
    }

    #[test]
    fn legacy_avatar_defaults_to_camera() {
        assert_eq!(AvatarKind::default(), AvatarKind::Camera);
        assert_eq!(serde_json::to_string(&AvatarKind::Fox).unwrap(), "\"fox\"");
    }

    #[test]
    fn tracking_cannot_poison_pose_with_nan() {
        let mut pose = Pose::default();
        pose.approach(
            Pose {
                yaw: f32::NAN,
                mouth: 20.,
                ..Pose::default()
            },
            1.,
        );
        assert_eq!(pose.yaw, 0.);
        assert_eq!(pose.mouth, 1.);
    }
}
