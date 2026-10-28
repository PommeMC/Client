//! `SnifferAnimation` keyframe tables, converted mechanically from the
//! decompiled definitions (`scaleVec` values stored as `v - 1`). The unused
//! `BABY_TRANSFORM` and `SNIFFER_BABY_FALL` are omitted: `SnifferRenderer`
//! bakes the baby layer into `SnifferModel`, so neither ever plays.

use super::{KfAnim, KfChannel, KfTarget, Kfi, kf};

pub(super) static SNIFFER_SNIFFSNIFF: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 8.0,
        looping: true,
        channels: &[KfChannel {
            part: "nose",
            target: KfTarget::Scale,
            frames: &[
                kf(0.0, 0.0, 0.0, 0.0, Lin),
                kf(0.5417, 0.0, 0.0, 0.0, Lin),
                kf(0.5833, 0.0, -0.5, 0.0, Cat),
                kf(0.6667, 0.0, 1.5, 0.0, Cat),
                kf(0.7917, 0.0, 0.0, 0.0, Cat),
                kf(0.9167, 0.0, 0.0, 0.0, Cat),
                kf(1.0, 0.0, 2.0, 0.0, Cat),
                kf(1.125, 0.0, 0.0, 0.0, Lin),
                kf(2.0, 0.0, 0.0, 0.0, Lin),
            ],
        }],
    }
};

pub(super) static SNIFFER_LONGSNIFF: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 1.0,
        looping: false,
        channels: &[
            KfChannel {
                part: "nose",
                target: KfTarget::Scale,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.0833, 0.0, -0.3, 0.0, Cat),
                    kf(0.125, 0.0, 2.0, 0.0, Cat),
                    kf(0.25, 0.0, 2.0, 0.0, Cat),
                    kf(0.7083, 0.0, 3.0, 0.0, Cat),
                    kf(0.8333, 0.0, 0.0, 0.0, Cat),
                    kf(1.0, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, -5.0, 0.0, 0.0, Lin),
                    kf(0.875, -20.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

pub(super) static SNIFFER_WALK: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.0,
        looping: true,
        channels: &[
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5833, 35.0, 0.0, 0.0, Lin),
                    kf(1.0, -35.0, 0.0, 0.0, Lin),
                    kf(1.1667, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 3.0, Lin),
                    kf(0.75, 0.0, 4.0, -1.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.1667, 0.0, 0.0, -1.0, Lin),
                    kf(2.0, 0.0, 0.0, 3.0, Lin),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -7.0, 0.0, 0.0, Lin),
                    kf(0.1667, -35.0, 0.0, 0.0, Lin),
                    kf(0.3333, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.1667, 0.0, 0.0, 0.0, Lin),
                    kf(1.75, 35.0, 0.0, 0.0, Lin),
                    kf(2.0, -7.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 2.67, -0.67, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.3333, 0.0, 0.0, -2.0, Lin),
                    kf(1.0, 0.0, 0.0, 2.0, Lin),
                    kf(1.1667, 0.0, 0.0, 3.0, Lin),
                    kf(1.9167, 0.0, 4.0, -1.0, Lin),
                    kf(2.0, 0.0, 2.67, -0.67, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5833, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 25.0, 0.0, 0.0, Lin),
                    kf(1.1667, 35.0, 0.0, 0.0, Lin),
                    kf(1.5833, -35.0, 0.0, 0.0, Lin),
                    kf(1.75, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, -0.5, Lin),
                    kf(0.5833, 0.0, 0.0, 2.0, Lin),
                    kf(1.0, 0.0, 2.22, 0.78, Lin),
                    kf(1.3333, 0.0, 4.0, -1.0, Lin),
                    kf(1.5833, 0.0, 0.0, 0.0, Lin),
                    kf(1.75, 0.0, 0.0, -2.0, Lin),
                    kf(2.0, 0.0, 0.0, -0.5, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -35.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.5833, 35.0, 0.0, 0.0, Lin),
                    kf(2.0, -35.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, -1.0, Lin),
                    kf(1.0, 0.0, 0.0, 3.0, Lin),
                    kf(1.75, 0.0, 4.0, -1.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 35.0, 0.0, 0.0, Lin),
                    kf(1.1667, -35.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 2.0, Lin),
                    kf(0.1667, 0.0, 0.0, 3.0, Lin),
                    kf(0.9167, 0.0, 4.0, -1.0, Lin),
                    kf(1.1667, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, -2.0, Lin),
                    kf(2.0, 0.0, 0.0, 2.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 25.0, 0.0, 0.0, Lin),
                    kf(0.1667, 35.0, 0.0, 0.0, Lin),
                    kf(0.5833, -35.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.5833, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 25.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 2.22, 0.78, Lin),
                    kf(0.3333, 0.0, 4.0, -1.0, Lin),
                    kf(0.5833, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, -2.0, Lin),
                    kf(1.0, 0.0, 0.0, -0.5, Lin),
                    kf(1.5833, 0.0, 0.0, 2.0, Lin),
                    kf(2.0, 0.0, 2.22, 0.78, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 1.0, 0.0, -2.5, Lin),
                    kf(0.5, -1.0, 0.0, 0.0, Lin),
                    kf(1.0, 1.0, 0.0, 2.5, Lin),
                    kf(1.5, -1.0, 0.0, 0.0, Lin),
                    kf(2.0, 1.0, 0.0, -2.5, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 0.0, 0.0, 0.0, Lin),
                    kf(0.375, 0.0, -1.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.375, 0.0, -1.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 7.5, 0.0, 0.0, Cat),
                    kf(0.1667, 9.0, 0.0, 0.0, Cat),
                    kf(0.875, -1.0, 0.0, 0.0, Cat),
                    kf(1.25, 7.0, 0.0, 0.0, Cat),
                    kf(1.75, 5.0, 0.0, 0.0, Cat),
                    kf(2.0, 7.5, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, -2.5, Cat),
                    kf(0.5, 0.0, 0.0, -7.5, Cat),
                    kf(1.0, 0.0, 0.0, -2.5, Cat),
                    kf(1.5, 0.0, 0.0, -7.5, Cat),
                    kf(2.0, 0.0, 0.0, -2.5, Cat),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 2.5, Cat),
                    kf(0.5, 0.0, 0.0, 7.5, Cat),
                    kf(1.0, 0.0, 0.0, 2.5, Cat),
                    kf(1.5, 0.0, 0.0, 7.5, Cat),
                    kf(2.0, 0.0, 0.0, 2.5, Cat),
                ],
            },
        ],
    }
};

pub(super) static SNIFFER_SNIFF_SEARCH: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.0,
        looping: true,
        channels: &[
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5833, 35.0, 0.0, 0.0, Lin),
                    kf(1.0, -35.0, 0.0, 0.0, Lin),
                    kf(1.1667, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 3.0, Lin),
                    kf(0.75, 0.0, 4.0, -1.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.1667, 0.0, 0.0, -1.0, Lin),
                    kf(2.0, 0.0, 0.0, 3.0, Lin),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -7.0, 0.0, 0.0, Lin),
                    kf(0.1667, -35.0, 0.0, 0.0, Lin),
                    kf(0.3333, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.1667, 0.0, 0.0, 0.0, Lin),
                    kf(1.75, 35.0, 0.0, 0.0, Lin),
                    kf(2.0, -7.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 2.67, -0.67, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.3333, 0.0, 0.0, -2.0, Lin),
                    kf(1.0, 0.0, 0.0, 2.0, Lin),
                    kf(1.1667, 0.0, 0.0, 3.0, Lin),
                    kf(1.9167, 0.0, 4.0, -1.0, Lin),
                    kf(2.0, 0.0, 2.67, -0.67, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5833, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 25.0, 0.0, 0.0, Lin),
                    kf(1.1667, 35.0, 0.0, 0.0, Lin),
                    kf(1.5833, -35.0, 0.0, 0.0, Lin),
                    kf(1.75, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, -0.5, Lin),
                    kf(0.5833, 0.0, 0.0, 2.0, Lin),
                    kf(1.0, 0.0, 2.22, 0.78, Lin),
                    kf(1.3333, 0.0, 4.0, -1.0, Lin),
                    kf(1.5833, 0.0, 0.0, 0.0, Lin),
                    kf(1.75, 0.0, 0.0, -2.0, Lin),
                    kf(2.0, 0.0, 0.0, -0.5, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -35.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.5833, 35.0, 0.0, 0.0, Lin),
                    kf(2.0, -35.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, -1.0, Lin),
                    kf(1.0, 0.0, 0.0, 3.0, Lin),
                    kf(1.75, 0.0, 4.0, -1.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 35.0, 0.0, 0.0, Lin),
                    kf(1.1667, -35.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 2.0, Lin),
                    kf(0.1667, 0.0, 0.0, 3.0, Lin),
                    kf(0.9167, 0.0, 4.0, -1.0, Lin),
                    kf(1.1667, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, -2.0, Lin),
                    kf(2.0, 0.0, 0.0, 2.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 25.0, 0.0, 0.0, Lin),
                    kf(0.1667, 35.0, 0.0, 0.0, Lin),
                    kf(0.5833, -35.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.5833, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 25.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 2.22, 0.78, Lin),
                    kf(0.3333, 0.0, 4.0, -1.0, Lin),
                    kf(0.5833, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, -2.0, Lin),
                    kf(1.0, 0.0, 0.0, -0.5, Lin),
                    kf(1.5833, 0.0, 0.0, 2.0, Lin),
                    kf(2.0, 0.0, 2.22, 0.78, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 2.5, 0.0, 0.0, Lin),
                    kf(0.5, 1.25, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 2.5, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 35.0, 0.0, 0.0, Lin),
                    kf(0.125, 33.61503, 11.46526, 9.803, Lin),
                    kf(0.875, 34.71128, 17.67415, 14.15251, Lin),
                    kf(1.125, 37.21128, -17.67415, -14.15251, Lin),
                    kf(1.875, 38.30529, -21.62827, -17.40292, Lin),
                    kf(2.0, 35.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, -2.0, 0.0, Lin), kf(2.0, 0.0, -2.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, -2.5, Lin),
                    kf(0.25, 0.0, 0.0, -15.0, Lin),
                    kf(0.5, 0.0, 0.0, -2.5, Lin),
                    kf(0.75, 0.0, 0.0, -15.0, Lin),
                    kf(1.0, 0.0, 0.0, -2.5, Lin),
                    kf(1.25, 0.0, 0.0, -15.0, Lin),
                    kf(1.5, 0.0, 0.0, -2.5, Lin),
                    kf(1.75, 0.0, 0.0, -15.0, Lin),
                    kf(2.0, 0.0, 0.0, -2.5, Lin),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 2.5, Lin),
                    kf(0.25, 0.0, 0.0, 15.0, Lin),
                    kf(0.5, 0.0, 0.0, 2.5, Lin),
                    kf(0.75, 0.0, 0.0, 15.0, Lin),
                    kf(1.0, 0.0, 0.0, 2.5, Lin),
                    kf(1.25, 0.0, 0.0, 15.0, Lin),
                    kf(1.5, 0.0, 0.0, 2.5, Lin),
                    kf(1.75, 0.0, 0.0, 15.0, Lin),
                    kf(2.0, 0.0, 0.0, 2.5, Lin),
                ],
            },
            KfChannel {
                part: "nose",
                target: KfTarget::Scale,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.0833, 0.0, 0.5, 0.0, Cat),
                    kf(0.2083, 0.0, 0.0, 0.0, Cat),
                    kf(0.375, 0.0, 0.0, 0.0, Cat),
                    kf(0.4583, 0.0, 1.5, 0.0, Cat),
                    kf(0.625, 0.0, 0.0, 0.0, Cat),
                    kf(0.8333, 0.0, 0.0, 0.0, Cat),
                    kf(0.9167, 0.0, 1.5, 0.0, Cat),
                    kf(1.0833, 0.0, 0.0, 0.0, Cat),
                    kf(1.2917, 0.0, 0.0, 0.0, Cat),
                    kf(1.3333, 0.0, 1.5, 0.0, Cat),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                    kf(1.625, 0.0, 0.0, 0.0, Cat),
                    kf(1.6667, 0.0, 2.5, 0.0, Cat),
                    kf(1.8333, 0.0, 0.0, 0.0, Cat),
                    kf(2.0, 0.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

pub(super) static SNIFFER_DIG: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 8.0,
        looping: false,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, 1.5, 0.0, 0.0, Lin),
                    kf(1.3333, -5.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                    kf(2.5, 2.5, 0.0, 0.0, Lin),
                    kf(3.0, 0.0, 0.0, 0.0, Lin),
                    kf(3.5, 2.5, 0.0, 0.0, Lin),
                    kf(4.0, 0.0, 0.0, 0.0, Lin),
                    kf(4.5, 2.5, 0.0, 0.0, Lin),
                    kf(5.6667, 5.0, 0.0, 0.0, Lin),
                    kf(5.8333, -2.5, 0.0, 0.0, Lin),
                    kf(6.0, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 1.0, 0.0, Lin),
                    kf(1.5, 0.0, -7.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Scale,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                    kf(1.5417, 0.04, -0.02, 0.02, Lin),
                    kf(1.5833, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(1.1667, 10.0, 0.0, 0.0, Cat),
                    kf(1.4167, -10.0, 0.0, 0.0, Cat),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                    kf(1.5833, 0.0, 0.0, 0.0, Cat),
                    kf(1.875, 0.0, 0.0, 0.0, Cat),
                    kf(2.0833, 0.0, 0.0, 0.0, Cat),
                    kf(2.5, 47.5, 0.0, 0.0, Cat),
                    kf(2.6667, 38.44, 0.0, 0.0, Cat),
                    kf(2.875, 10.95951, 13.57454, -14.93501, Cat),
                    kf(3.2083, 47.5, 0.0, 0.0, Cat),
                    kf(3.5833, 55.0, 0.0, 0.0, Cat),
                    kf(3.7917, 4.2932, -16.187, 10.90042, Cat),
                    kf(4.125, 47.5, 0.0, 0.0, Cat),
                    kf(4.4167, 54.71135, 7.98009, -5.56662, Cat),
                    kf(4.5, 55.72895, -6.77684, 4.46197, Cat),
                    kf(4.5833, 54.71135, 7.98009, -5.56662, Cat),
                    kf(4.6667, 55.72895, -6.77684, 4.46197, Cat),
                    kf(4.75, 54.71135, 7.98009, -5.56662, Cat),
                    kf(4.8333, 55.72895, -6.77684, 4.46197, Cat),
                    kf(5.0, 65.0, 0.0, 0.0, Cat),
                    kf(5.75, 65.0, 0.0, 0.0, Cat),
                    kf(5.9167, -32.5, 0.0, 0.0, Cat),
                    kf(6.25, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.625, 0.0, 0.0, 0.0, Lin),
                    kf(1.375, 0.0, 1.0, 0.0, Lin),
                    kf(1.5, 0.0, 1.0, 0.0, Lin),
                    kf(1.5833, 0.0, 1.0, 0.0, Lin),
                    kf(1.875, 0.0, 1.0, 0.0, Lin),
                    kf(2.0833, 0.0, 3.0, 0.0, Lin),
                    kf(2.2917, 0.0, 6.0, 0.0, Lin),
                    kf(2.6667, 0.0, 0.0, 0.0, Lin),
                    kf(3.2083, 0.0, 4.0, 0.0, Lin),
                    kf(3.5833, 0.0, 0.0, 0.0, Lin),
                    kf(4.125, 0.0, 4.0, 0.0, Lin),
                    kf(5.0, 0.0, 0.0, 0.0, Lin),
                    kf(5.75, 0.0, 1.0, 0.0, Lin),
                    kf(6.0, 0.0, 1.5, 0.0, Lin),
                    kf(6.25, 0.0, 1.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, -2.5, Lin),
                    kf(1.25, 0.0, 0.0, -2.5, Lin),
                    kf(1.4167, 0.0, 0.0, -50.0, Lin),
                    kf(1.5833, 0.0, 0.0, -30.0, Lin),
                    kf(5.9167, 0.0, 0.0, -30.0, Lin),
                    kf(6.0833, 0.0, 0.0, -65.0, Lin),
                    kf(6.3333, 0.0, 0.0, -30.0, Lin),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 2.5, Lin),
                    kf(1.25, 0.0, 0.0, 2.5, Lin),
                    kf(1.4167, 0.0, 0.0, 50.0, Lin),
                    kf(1.5833, 0.0, 0.0, 30.0, Lin),
                    kf(5.9167, 0.0, 0.0, 30.0, Lin),
                    kf(6.0833, 0.0, 0.0, 65.0, Lin),
                    kf(6.3333, 0.0, 0.0, 30.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.375, 0.0, 0.0, 90.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.2917, -2.0, -0.75, 0.0, Lin),
                    kf(1.375, -4.0, -5.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.25, 0.0, 0.0, 0.0, Lin),
                    kf(1.4167, 0.0, 0.0, 90.0, Lin),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.25, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, -2.0, -0.75, 0.0, Lin),
                    kf(1.4167, -4.0, -5.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 90.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, 0.0, Lin),
                    kf(1.4167, -2.0, -0.75, 0.0, Lin),
                    kf(1.5, -4.0, -5.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.375, 0.0, 0.0, -90.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.2917, 2.0, -0.75, 0.0, Lin),
                    kf(1.375, 4.0, -5.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.25, 0.0, 0.0, 0.0, Lin),
                    kf(1.4167, 0.0, 0.0, -90.0, Lin),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.25, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 2.0, -0.75, 0.0, Lin),
                    kf(1.4167, 4.0, -5.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, -90.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(1.3333, 0.0, 0.0, 0.0, Lin),
                    kf(1.4167, 2.0, -0.75, 0.0, Lin),
                    kf(1.5, 4.0, -5.5, 0.0, Lin),
                ],
            },
        ],
    }
};

pub(super) static SNIFFER_STAND_UP: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 3.0,
        looping: false,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 2.5, 0.0, 0.0, Lin),
                    kf(1.5, -2.5, 0.0, 0.0, Lin),
                    kf(1.7083, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.25, 0.0, -7.0, 0.0, Lin),
                    kf(0.75, 0.0, -7.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                    kf(1.7083, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.3333, -5.0, 0.0, 0.0, Lin),
                    kf(0.7083, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 10.0, 0.0, 0.0, Lin),
                    kf(1.375, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 1.0, 0.0, Lin), kf(1.375, 0.0, 1.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, -30.0, Lin),
                    kf(0.9167, 0.0, 0.0, -30.0, Lin),
                    kf(1.2083, 0.0, 0.0, -5.0, Lin),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 30.0, Lin),
                    kf(0.9167, 0.0, 0.0, 30.0, Lin),
                    kf(1.2083, 0.0, 0.0, 5.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 90.0, Cat), kf(0.4583, 0.0, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -4.0, -5.5, 0.0, Cat),
                    kf(0.2083, 6.0, -5.5, 0.0, Cat),
                    kf(0.4583, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0833, 0.0, 0.0, 90.0, Cat),
                    kf(0.5833, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0833, -4.0, -5.5, 0.0, Cat),
                    kf(0.3333, 6.0, -5.5, 0.0, Cat),
                    kf(0.5833, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.1667, 0.0, 0.0, 90.0, Cat),
                    kf(0.6667, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.1667, -4.0, -5.5, 0.0, Cat),
                    kf(0.4167, 6.0, -5.5, 0.0, Cat),
                    kf(0.6667, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, -90.0, Cat),
                    kf(0.4583, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 4.0, -5.5, 0.0, Cat),
                    kf(0.2083, -6.0, -5.5, 0.0, Cat),
                    kf(0.4583, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0833, 0.0, 0.0, -90.0, Cat),
                    kf(0.5833, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_mid_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0833, 4.0, -5.5, 0.0, Cat),
                    kf(0.3333, -6.0, -5.5, 0.0, Cat),
                    kf(0.5833, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.1667, 0.0, 0.0, -90.0, Cat),
                    kf(0.6667, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.1667, 4.0, -5.5, 0.0, Cat),
                    kf(0.4167, -6.0, -5.5, 0.0, Cat),
                    kf(0.6667, 0.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

pub(super) static SNIFFER_HAPPY: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.0,
        looping: true,
        channels: &[
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, -32.00206, 19.3546, -11.70092, Cat),
                    kf(1.0, 0.0, 0.0, 0.0, Cat),
                    kf(1.5, -32.00206, -19.3546, 11.70092, Cat),
                    kf(2.0, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.5, 0.0, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, -67.5, Cat),
                    kf(0.9583, 0.0, 0.0, 0.0, Cat),
                    kf(1.125, 0.0, 0.0, -67.5, Cat),
                    kf(1.2917, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.5, 0.0, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 67.5, Cat),
                    kf(0.9583, 0.0, 0.0, 0.0, Cat),
                    kf(1.125, 0.0, 0.0, 67.5, Cat),
                    kf(1.2917, 0.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins each table's shape against `SnifferAnimation.java`, so a bad
    /// regeneration fails: (length, looping, channels, keyframes).
    #[test]
    fn tables_match_the_vanilla_definitions() {
        for (anim, length, looping, channels, frames) in [
            (&SNIFFER_SNIFFSNIFF, 8.0, true, 1, 9),
            (&SNIFFER_LONGSNIFF, 1.0, false, 2, 11),
            (&SNIFFER_WALK, 2.0, true, 17, 102),
            (&SNIFFER_SNIFF_SEARCH, 2.0, true, 18, 120),
            (&SNIFFER_DIG, 8.0, false, 19, 116),
            (&SNIFFER_STAND_UP, 3.0, false, 18, 51),
            (&SNIFFER_HAPPY, 2.0, true, 3, 15),
        ] {
            assert_eq!(anim.length, length);
            assert_eq!(anim.looping, looping);
            assert_eq!(anim.channels.len(), channels);
            let total: usize = anim.channels.iter().map(|c| c.frames.len()).sum();
            assert_eq!(total, frames);
            for c in anim.channels {
                assert!(c.frames.windows(2).all(|w| w[0].t <= w[1].t));
            }
        }
    }
}
