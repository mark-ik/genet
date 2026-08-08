//! Color spaces and the conversions between them.
//!
//! Harvest lift (cutover plan F0, harvest plan fork-and-own rules). Algorithms,
//! matrices, and constants come from CSS Color 4's conversion code
//! (<https://drafts.csswg.org/css-color-4/#color-conversion-code>) by way of the
//! stylo fork's `style/color/convert.rs` (mark-ik/stylo, branch genet-rename,
//! rev b157d925267fdd37b03f43e3387ab2f0909e57b0), MPL-2.0.
//!
//! Two deliberate departures from the donor:
//!
//! - No `euclid`. The donor's matrices are pre-transposed for
//!   `Transform3D::transform_vector3d`; these are written in the spec's own
//!   row-major order and multiplied directly, so they can be diffed against
//!   the spec text without mentally transposing.
//! - Missing components are carried as NaN, as in the donor, but every public
//!   entry point that leaves this module resolves them. NaN is an in-module
//!   representation, never a value a consumer sees.

use std::fmt;

use crate::values::ParseError;

mod perceptual;
mod rgb;

use perceptual::{
    lab_from_xyz, lab_to_xyz, oklab_from_xyz, oklab_to_xyz, orthogonal_to_polar,
    polar_to_orthogonal,
};
use rgb::{
    A98_TO_XYZ, P3_TO_XYZ, PROPHOTO_TO_XYZ, REC2020_TO_XYZ, SRGB_TO_XYZ, XYZ_TO_A98, XYZ_TO_P3,
    XYZ_TO_PROPHOTO, XYZ_TO_REC2020, XYZ_TO_SRGB, a98_to_linear, hsl_to_rgb, hwb_to_rgb,
    linear_to_a98, linear_to_prophoto, linear_to_rec2020, linear_to_srgb, prophoto_to_linear,
    rec2020_to_linear, rgb_from_xyz, rgb_to_hsl, rgb_to_hwb, rgb_to_xyz, srgb_to_linear,
};

/// Three color channels. Missing components (CSS `none`) are NaN.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Components(pub f32, pub f32, pub f32);

impl Components {
    pub fn map(self, mut f: impl FnMut(f32) -> f32) -> Self {
        Self(f(self.0), f(self.1), f(self.2))
    }

    /// Missing components become zero. CSS Color 4 requires this before any
    /// conversion: `none` participates as 0 once a color leaves its own space.
    pub fn resolve_missing(self) -> Self {
        self.map(|v| if v.is_nan() { 0.0 } else { v })
    }

    fn mul(self, other: Self) -> Self {
        Self(self.0 * other.0, self.1 * other.1, self.2 * other.2)
    }

    fn div(self, other: Self) -> Self {
        Self(self.0 / other.0, self.1 / other.1, self.2 / other.2)
    }
}

/// Row-major 3x3, in the spec's own orientation:
/// `out.0 = m[0][0] * x + m[0][1] * y + m[0][2] * z`.
pub(super) type Matrix = [[f32; 3]; 3];

pub(super) fn transform(from: Components, m: &Matrix) -> Components {
    let Components(x, y, z) = from;
    Components(
        m[0][0] * x + m[0][1] * y + m[0][2] * z,
        m[1][0] * x + m[1][1] * y + m[1][2] * z,
        m[2][0] * x + m[2][1] * y + m[2][2] * z,
    )
}

/// Normalize hue into [0, 360).
pub fn normalize_hue(hue: f32) -> f32 {
    hue - 360.0 * (hue / 360.0).floor()
}

fn epsilon_for_range(min: f32, max: f32) -> f32 {
    (max - min) / 1.0e5
}

/// The reference white a space is defined against.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WhitePoint {
    D50,
    D65,
}

impl WhitePoint {
    pub(super) const fn values(self) -> Components {
        match self {
            Self::D50 => Components(0.9642956764295677, 1.0, 0.8251046025104602),
            Self::D65 => Components(0.9504559270516716, 1.0, 1.0890577507598784),
        }
    }
}

const D65_TO_D50: Matrix = [
    [
        1.0479298208405488,
        0.022946793341019088,
        -0.05019222954313557,
    ],
    [
        0.029627815688159344,
        0.990434484573249,
        -0.01707382502938514,
    ],
    [
        -0.009243058152591178,
        0.015055144896577895,
        0.7518742899580008,
    ],
];

const D50_TO_D65: Matrix = [
    [
        0.9554734527042182,
        -0.023098536874261423,
        0.0632593086610217,
    ],
    [
        -0.028369706963208136,
        1.0099954580058226,
        0.021041398966943008,
    ],
    [
        0.012314001688319899,
        -0.020507696433477912,
        1.3303659366080753,
    ],
];

fn adapt_white_point(from: WhitePoint, to: WhitePoint, xyz: Components) -> Components {
    match (from, to) {
        (WhitePoint::D50, WhitePoint::D65) => transform(xyz, &D50_TO_D65),
        (WhitePoint::D65, WhitePoint::D50) => transform(xyz, &D65_TO_D50),
        _ => xyz,
    }
}

/// Every color space Livery can hold a color in.
///
/// `Hsl` and `Hwb` are here because CSS Color 4 interpolates and serializes
/// them as their own spaces even though they are alternate notations for sRGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    Srgb,
    SrgbLinear,
    Hsl,
    Hwb,
    Lab,
    Lch,
    Oklab,
    Oklch,
    DisplayP3,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    XyzD50,
    XyzD65,
}

impl ColorSpace {
    /// The `color()` function's predefined spaces. Other spaces have their own
    /// function syntax and are not valid inside `color()`.
    pub fn from_predefined_name(name: &str) -> Option<Self> {
        Some(match_ignore_ascii_case(
            name,
            &[
                ("srgb", Self::Srgb),
                ("srgb-linear", Self::SrgbLinear),
                ("display-p3", Self::DisplayP3),
                ("a98-rgb", Self::A98Rgb),
                ("prophoto-rgb", Self::ProphotoRgb),
                ("rec2020", Self::Rec2020),
                ("xyz", Self::XyzD65),
                ("xyz-d50", Self::XyzD50),
                ("xyz-d65", Self::XyzD65),
            ],
        )?)
    }

    /// Every space nameable as a `color-mix()` interpolation space.
    pub fn from_interpolation_name(name: &str) -> Option<Self> {
        match_ignore_ascii_case(
            name,
            &[
                ("srgb", Self::Srgb),
                ("srgb-linear", Self::SrgbLinear),
                ("hsl", Self::Hsl),
                ("hwb", Self::Hwb),
                ("lab", Self::Lab),
                ("lch", Self::Lch),
                ("oklab", Self::Oklab),
                ("oklch", Self::Oklch),
                ("display-p3", Self::DisplayP3),
                ("a98-rgb", Self::A98Rgb),
                ("prophoto-rgb", Self::ProphotoRgb),
                ("rec2020", Self::Rec2020),
                ("xyz", Self::XyzD65),
                ("xyz-d50", Self::XyzD50),
                ("xyz-d65", Self::XyzD65),
            ],
        )
    }

    /// The CSS name used when serializing a `color()` value.
    pub fn predefined_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Srgb => "srgb",
            Self::SrgbLinear => "srgb-linear",
            Self::DisplayP3 => "display-p3",
            Self::A98Rgb => "a98-rgb",
            Self::ProphotoRgb => "prophoto-rgb",
            Self::Rec2020 => "rec2020",
            Self::XyzD50 => "xyz-d50",
            Self::XyzD65 => "xyz-d65",
            _ => return None,
        })
    }

    /// Whether the third component is a hue angle, which interpolates the
    /// short way round rather than linearly.
    pub fn hue_index(self) -> Option<usize> {
        match self {
            Self::Hsl | Self::Hwb => Some(0),
            Self::Lch | Self::Oklch => Some(2),
            _ => None,
        }
    }

    /// Whether `color()` may name this space, which also decides serialization
    /// form: `color(space c1 c2 c3)` versus `lab(...)`, `oklch(...)`, and so on.
    pub fn is_predefined(self) -> bool {
        self.predefined_name().is_some()
    }

    fn white_point(self) -> WhitePoint {
        match self {
            Self::Lab | Self::Lch | Self::ProphotoRgb | Self::XyzD50 => WhitePoint::D50,
            _ => WhitePoint::D65,
        }
    }

    /// Convert these components into XYZ at this space's own white point.
    fn to_xyz(self, from: Components) -> Components {
        match self {
            Self::XyzD50 | Self::XyzD65 => from,
            Self::Srgb => rgb_to_xyz(srgb_to_linear(from), &SRGB_TO_XYZ),
            Self::SrgbLinear => rgb_to_xyz(from, &SRGB_TO_XYZ),
            Self::Hsl => rgb_to_xyz(srgb_to_linear(hsl_to_rgb(from)), &SRGB_TO_XYZ),
            Self::Hwb => rgb_to_xyz(srgb_to_linear(hwb_to_rgb(from)), &SRGB_TO_XYZ),
            Self::DisplayP3 => rgb_to_xyz(srgb_to_linear(from), &P3_TO_XYZ),
            Self::A98Rgb => rgb_to_xyz(a98_to_linear(from), &A98_TO_XYZ),
            Self::ProphotoRgb => rgb_to_xyz(prophoto_to_linear(from), &PROPHOTO_TO_XYZ),
            Self::Rec2020 => rgb_to_xyz(rec2020_to_linear(from), &REC2020_TO_XYZ),
            Self::Lab => lab_to_xyz(from),
            Self::Lch => lab_to_xyz(polar_to_orthogonal(from)),
            Self::Oklab => oklab_to_xyz(from),
            Self::Oklch => oklab_to_xyz(polar_to_orthogonal(from)),
        }
    }

    /// Convert XYZ at this space's own white point back into these components.
    fn from_xyz(self, xyz: Components) -> Components {
        match self {
            Self::XyzD50 | Self::XyzD65 => xyz,
            Self::Srgb => linear_to_srgb(rgb_from_xyz(xyz, &XYZ_TO_SRGB)),
            Self::SrgbLinear => rgb_from_xyz(xyz, &XYZ_TO_SRGB),
            Self::Hsl => rgb_to_hsl(linear_to_srgb(rgb_from_xyz(xyz, &XYZ_TO_SRGB))),
            Self::Hwb => rgb_to_hwb(linear_to_srgb(rgb_from_xyz(xyz, &XYZ_TO_SRGB))),
            Self::DisplayP3 => linear_to_srgb(rgb_from_xyz(xyz, &XYZ_TO_P3)),
            Self::A98Rgb => linear_to_a98(rgb_from_xyz(xyz, &XYZ_TO_A98)),
            Self::ProphotoRgb => linear_to_prophoto(rgb_from_xyz(xyz, &XYZ_TO_PROPHOTO)),
            Self::Rec2020 => linear_to_rec2020(rgb_from_xyz(xyz, &XYZ_TO_REC2020)),
            Self::Lab => lab_from_xyz(xyz),
            Self::Lch => orthogonal_to_polar(lab_from_xyz(xyz), epsilon_for_range(0.0, 100.0)),
            Self::Oklab => oklab_from_xyz(xyz),
            Self::Oklch => orthogonal_to_polar(oklab_from_xyz(xyz), epsilon_for_range(0.0, 1.0)),
        }
    }

    /// Convert components from this space into `to`.
    ///
    /// Missing components resolve to zero first, per CSS Color 4: `none` only
    /// survives while a color stays in its own space.
    pub fn convert(self, to: Self, from: Components) -> Components {
        if self == to {
            return from;
        }
        let from = from.resolve_missing();
        let xyz = self.to_xyz(from);
        let xyz = adapt_white_point(self.white_point(), to.white_point(), xyz);
        to.from_xyz(xyz)
    }
}

fn match_ignore_ascii_case<T: Copy>(name: &str, table: &[(&str, T)]) -> Option<T> {
    table
        .iter()
        .find(|(key, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| *value)
}

impl fmt::Display for ColorSpace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Srgb => "srgb",
            Self::SrgbLinear => "srgb-linear",
            Self::Hsl => "hsl",
            Self::Hwb => "hwb",
            Self::Lab => "lab",
            Self::Lch => "lch",
            Self::Oklab => "oklab",
            Self::Oklch => "oklch",
            Self::DisplayP3 => "display-p3",
            Self::A98Rgb => "a98-rgb",
            Self::ProphotoRgb => "prophoto-rgb",
            Self::Rec2020 => "rec2020",
            Self::XyzD50 => "xyz-d50",
            Self::XyzD65 => "xyz-d65",
        })
    }
}

impl std::str::FromStr for ColorSpace {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::from_interpolation_name(input).ok_or_else(|| ParseError::expected("a color space"))
    }
}
