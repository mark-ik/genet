/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The RGB-family spaces: transfer functions, primaries, HSL, and HWB.
//!
//! Split out of `space.rs` to keep each file under the repo's size
//! ceiling. Provenance is the parent module's: CSS Color 4's conversion
//! code by way of the stylo fork's `style/color/convert.rs`.

use super::{Components, Matrix, normalize_hue, transform};

// ── RGB-family transfer functions ────────────────────────────────────────

pub(super) fn srgb_to_linear(from: Components) -> Components {
    from.map(|value| {
        let abs = value.abs();
        if abs < 0.04045 {
            value / 12.92
        } else {
            value.signum() * ((abs + 0.055) / 1.055).powf(2.4)
        }
    })
}

pub(super) fn linear_to_srgb(from: Components) -> Components {
    from.map(|value| {
        let abs = value.abs();
        if abs > 0.0031308 {
            value.signum() * (1.055 * abs.powf(1.0 / 2.4) - 0.055)
        } else {
            12.92 * value
        }
    })
}

pub(super) fn a98_to_linear(from: Components) -> Components {
    from.map(|v| v.signum() * v.abs().powf(2.19921875))
}

pub(super) fn linear_to_a98(from: Components) -> Components {
    from.map(|v| v.signum() * v.abs().powf(0.4547069271758437))
}

pub(super) fn prophoto_to_linear(from: Components) -> Components {
    const ET2: f32 = 16.0 / 512.0;
    from.map(|value| {
        let abs = value.abs();
        if abs <= ET2 {
            value / 16.0
        } else {
            value.signum() * abs.powf(1.8)
        }
    })
}

pub(super) fn linear_to_prophoto(from: Components) -> Components {
    const ET: f32 = 1.0 / 512.0;
    from.map(|value| {
        let abs = value.abs();
        if abs >= ET {
            value.signum() * abs.powf(1.0 / 1.8)
        } else {
            16.0 * value
        }
    })
}

const REC2020_ALPHA: f32 = 1.09929682680944;
const REC2020_BETA: f32 = 0.018053968510807;

pub(super) fn rec2020_to_linear(from: Components) -> Components {
    from.map(|value| {
        let abs = value.abs();
        if abs < REC2020_BETA * 4.5 {
            value / 4.5
        } else {
            value.signum() * ((abs + REC2020_ALPHA - 1.0) / REC2020_ALPHA).powf(1.0 / 0.45)
        }
    })
}

pub(super) fn linear_to_rec2020(from: Components) -> Components {
    from.map(|value| {
        let abs = value.abs();
        if abs > REC2020_BETA {
            value.signum() * (REC2020_ALPHA * abs.powf(0.45) - (REC2020_ALPHA - 1.0))
        } else {
            4.5 * value
        }
    })
}

pub(super) fn rgb_to_xyz(linear: Components, matrix: &Matrix) -> Components {
    transform(linear, matrix)
}

pub(super) fn rgb_from_xyz(xyz: Components, matrix: &Matrix) -> Components {
    transform(xyz, matrix)
}

// ── RGB primaries ────────────────────────────────────────────────────────

pub(super) const SRGB_TO_XYZ: Matrix = [
    [0.4123907992659595, 0.35758433938387796, 0.1804807884018343],
    [0.21263900587151036, 0.7151686787677559, 0.07219231536073371],
    [0.01933081871559185, 0.11919477979462599, 0.9505321522496606],
];

pub(super) const XYZ_TO_SRGB: Matrix = [
    [3.2409699419045213, -1.5373831775700935, -0.4986107602930033],
    [-0.9692436362808798, 1.8759675015077206, 0.04155505740717561],
    [
        0.05563007969699361,
        -0.20397695888897657,
        1.0569715142428786,
    ],
];

pub(super) const P3_TO_XYZ: Matrix = [
    [0.48657094864821626, 0.26566769316909294, 0.1982172852343625],
    [0.22897456406974884, 0.6917385218365062, 0.079286914093745],
    [0.0, 0.045113381858902575, 1.0439443689009757],
];

pub(super) const XYZ_TO_P3: Matrix = [
    [
        2.4934969119414245,
        -0.9313836179191236,
        -0.40271078445071684,
    ],
    [-0.829488969561575, 1.7626640603183468, 0.02362468584194359],
    [
        0.035845830243784335,
        -0.07617238926804171,
        0.9568845240076873,
    ],
];

pub(super) const A98_TO_XYZ: Matrix = [
    [0.5766690429101308, 0.18555823790654627, 0.18822864623499472],
    [0.29734497525053616, 0.627363566255466, 0.07529145849399789],
    [
        0.027031361386412378,
        0.07068885253582714,
        0.9913375368376389,
    ],
];

pub(super) const XYZ_TO_A98: Matrix = [
    [2.041587903810746, -0.5650069742788596, -0.3447313507783295],
    [-0.9692436362808798, 1.8759675015077206, 0.04155505740717561],
    [
        0.013444280632031024,
        -0.11836239223101824,
        1.0151749943912054,
    ],
];

pub(super) const PROPHOTO_TO_XYZ: Matrix = [
    [0.7977604896723027, 0.13518583717574031, 0.0313493495815248],
    [
        0.2880711282292934,
        0.7118432178101014,
        0.00008565396060525902,
    ],
    [0.0, 0.0, 0.8251046025104601],
];

pub(super) const XYZ_TO_PROPHOTO: Matrix = [
    [
        1.3457989731028281,
        -0.25558010007997534,
        -0.05110628506753401,
    ],
    [-0.5446224939028347, 1.5082327413132781, 0.02053603239147973],
    [0.0, 0.0, 1.2119675456389454],
];

pub(super) const REC2020_TO_XYZ: Matrix = [
    [0.6369580483012913, 0.14461690358620838, 0.16888097516417205],
    [0.26270021201126703, 0.677998071518871, 0.059301716469861945],
    [0.0, 0.028072693049087508, 1.0609850577107909],
];

pub(super) const XYZ_TO_REC2020: Matrix = [
    [1.7166511879712676, -0.3556707837763924, -0.2533662813736598],
    [-0.666684351832489, 1.616481236634939, 0.01576854581391113],
    [
        0.017639857445310915,
        -0.042770613257808655,
        0.942103121235474,
    ],
];

// ── HSL and HWB ──────────────────────────────────────────────────────────

fn rgb_to_hue_min_max(red: f32, green: f32, blue: f32) -> (f32, f32, f32) {
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let hue = if delta != 0.0 {
        60.0 * if max == red {
            (green - blue) / delta + if green < blue { 6.0 } else { 0.0 }
        } else if max == green {
            (blue - red) / delta + 2.0
        } else {
            (red - green) / delta + 4.0
        }
    } else {
        f32::NAN
    };
    (hue, min, max)
}

/// <https://drafts.csswg.org/css-color-4/#hsl-to-rgb>
pub(super) fn hsl_to_rgb(from: Components) -> Components {
    fn hue_to_rgb(t1: f32, t2: f32, hue: f32) -> f32 {
        let hue = normalize_hue(hue);
        if hue * 6.0 < 360.0 {
            t1 + (t2 - t1) * hue / 60.0
        } else if hue * 2.0 < 360.0 {
            t2
        } else if hue * 3.0 < 720.0 {
            t1 + (t2 - t1) * (240.0 - hue) / 60.0
        } else {
            t1
        }
    }

    let Components(hue, saturation, lightness) = from.resolve_missing();
    let saturation = saturation / 100.0;
    let lightness = lightness / 100.0;
    let t2 = if lightness <= 0.5 {
        lightness * (saturation + 1.0)
    } else {
        lightness + saturation - lightness * saturation
    };
    let t1 = lightness * 2.0 - t2;
    Components(
        hue_to_rgb(t1, t2, hue + 120.0),
        hue_to_rgb(t1, t2, hue),
        hue_to_rgb(t1, t2, hue - 120.0),
    )
}

/// <https://drafts.csswg.org/css-color-4/#rgb-to-hsl>
pub(super) fn rgb_to_hsl(from: Components) -> Components {
    let Components(red, green, blue) = from;
    let (hue, min, max) = rgb_to_hue_min_max(red, green, blue);
    let lightness = (min + max) / 2.0;
    let delta = max - min;
    let saturation = if delta != 0.0 {
        if lightness == 0.0 || lightness == 1.0 {
            0.0
        } else {
            (max - lightness) / lightness.min(1.0 - lightness)
        }
    } else {
        0.0
    };
    Components(hue, saturation * 100.0, lightness * 100.0)
}

/// <https://drafts.csswg.org/css-color-4/#hwb-to-rgb>
pub(super) fn hwb_to_rgb(from: Components) -> Components {
    let Components(hue, whiteness, blackness) = from.resolve_missing();
    let whiteness = whiteness / 100.0;
    let blackness = blackness / 100.0;
    if whiteness + blackness >= 1.0 {
        let gray = whiteness / (whiteness + blackness);
        return Components(gray, gray, gray);
    }
    let x = 1.0 - whiteness - blackness;
    hsl_to_rgb(Components(hue, 100.0, 50.0)).map(|v| v * x + whiteness)
}

/// <https://drafts.csswg.org/css-color-4/#rgb-to-hwb>
pub(super) fn rgb_to_hwb(from: Components) -> Components {
    let Components(red, green, blue) = from;
    let (hue, min, max) = rgb_to_hue_min_max(red, green, blue);
    Components(hue, min * 100.0, (1.0 - max) * 100.0)
}
