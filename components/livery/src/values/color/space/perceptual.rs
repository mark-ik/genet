/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The perceptual spaces: Lab, LCH, Oklab, and OkLCH.
//!
//! Split out of `space.rs` to keep each file under the repo's size
//! ceiling. Provenance is the parent module's.

use super::{Components, Matrix, WhitePoint, normalize_hue, transform};

// ── Lab, LCH, Oklab, OkLCH ───────────────────────────────────────────────

/// <https://drafts.csswg.org/css-color-4/#lab-to-lch>
pub(super) fn orthogonal_to_polar(from: Components, epsilon: f32) -> Components {
    let Components(lightness, a, b) = from;
    let chroma = (a * a + b * b).sqrt();
    // Near the achromatic axis the hue angle swings wildly and is essentially
    // random, so the spec treats it as powerless, which is to say missing.
    let hue = if (a.abs() < epsilon && b.abs() < epsilon) || chroma.abs() < epsilon {
        f32::NAN
    } else {
        normalize_hue(b.atan2(a).to_degrees())
    };
    Components(lightness, chroma, hue)
}

/// <https://drafts.csswg.org/css-color-4/#lch-to-lab>
pub(super) fn polar_to_orthogonal(from: Components) -> Components {
    let Components(lightness, chroma, hue) = from;
    if hue.is_nan() {
        return Components(lightness, 0.0, 0.0);
    }
    let hue = hue.to_radians();
    Components(lightness, chroma * hue.cos(), chroma * hue.sin())
}

const LAB_KAPPA: f32 = 24389.0 / 27.0;
const LAB_EPSILON: f32 = 216.0 / 24389.0;

pub(super) fn lab_to_xyz(from: Components) -> Components {
    let Components(lightness, a, b) = from;
    let f1 = (lightness + 16.0) / 116.0;
    let f0 = f1 + a / 500.0;
    let f2 = f1 - b / 200.0;

    let f0_cubed = f0 * f0 * f0;
    let x = if f0_cubed > LAB_EPSILON {
        f0_cubed
    } else {
        (116.0 * f0 - 16.0) / LAB_KAPPA
    };
    let y = if lightness > LAB_KAPPA * LAB_EPSILON {
        let v = (lightness + 16.0) / 116.0;
        v * v * v
    } else {
        lightness / LAB_KAPPA
    };
    let f2_cubed = f2 * f2 * f2;
    let z = if f2_cubed > LAB_EPSILON {
        f2_cubed
    } else {
        (116.0 * f2 - 16.0) / LAB_KAPPA
    };

    Components(x, y, z).mul(WhitePoint::D50.values())
}

pub(super) fn lab_from_xyz(from: Components) -> Components {
    let adapted = from.div(WhitePoint::D50.values());
    let Components(f0, f1, f2) = adapted.map(|v| {
        if v > LAB_EPSILON {
            v.cbrt()
        } else {
            (LAB_KAPPA * v + 16.0) / 116.0
        }
    });
    Components(116.0 * f1 - 16.0, 500.0 * (f0 - f1), 200.0 * (f1 - f2))
}

const OKLAB_TO_LMS: Matrix = [
    [0.99999999845051981432, 0.39633779217376785678, 0.21580375806075880339],
    [1.0000000088817607767, -0.1055613423236563494, -0.063854174771705903402],
    [1.0000000546724109177, -0.089484182094965759684, -1.2914855378640917399],
];

const LMS_TO_XYZ: Matrix = [
    [1.2268798733741557, -0.5578149965554813, 0.28139105017721583],
    [-0.04057576262431372, 1.1122868293970594, -0.07171106666151701],
    [-0.07637294974672142, -0.4214933239627914, 1.5869240244272418],
];

const XYZ_TO_LMS: Matrix = [
    [0.8190224432164319, 0.3619062562801221, -0.12887378261216414],
    [0.0329836671980271, 0.9292868468965546, 0.03614466816999844],
    [0.048177199566046255, 0.26423952494422764, 0.6335478258136937],
];

const LMS_TO_OKLAB: Matrix = [
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
];

pub(super) fn oklab_to_xyz(from: Components) -> Components {
    let lms = transform(from, &OKLAB_TO_LMS).map(|v| v * v * v);
    transform(lms, &LMS_TO_XYZ)
}

pub(super) fn oklab_from_xyz(from: Components) -> Components {
    let lms = transform(from, &XYZ_TO_LMS).map(f32::cbrt);
    transform(lms, &LMS_TO_OKLAB)
}
