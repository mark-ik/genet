/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Dimensional parsing and reduction for CSS math expressions.
//!
//! Harvested and reshaped from Stylo's precedence and dimensional-arithmetic
//! model in `style/values/specified/calc.rs` at
//! `b157d925267fdd37b03f43e3387ab2f0909e57b0`. Livery keeps only the stable
//! length-percentage subset needed by its current property lane: nested
//! `calc()`, parentheses, sums, products, and division by a number.

use cssparser::{Parser, ParserInput, Token};

use super::{CalcLengthPercentage, LengthUnit, ParseError};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Numeric {
    Number(f32),
    LengthPercentage(CalcLengthPercentage),
}

impl Numeric {
    fn sum(self, other: Self) -> Result<Self, ()> {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => finite(left + right).map(Self::Number),
            (Self::LengthPercentage(left), Self::LengthPercentage(right)) => {
                let value = CalcLengthPercentage {
                    percentage: left.percentage + right.percentage,
                    px: left.px + right.px,
                    em: left.em + right.em,
                    rem: left.rem + right.rem,
                };
                value
                    .is_finite()
                    .then_some(Self::LengthPercentage(value))
                    .ok_or(())
            },
            _ => Err(()),
        }
    }

    fn negate(self) -> Result<Self, ()> {
        self.scale(-1.0)
    }

    fn product(self, other: Self) -> Result<Self, ()> {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => finite(left * right).map(Self::Number),
            (Self::Number(number), Self::LengthPercentage(value))
            | (Self::LengthPercentage(value), Self::Number(number)) => {
                value.scale(number).map(Self::LengthPercentage)
            },
            (Self::LengthPercentage(_), Self::LengthPercentage(_)) => Err(()),
        }
    }

    fn divide(self, other: Self) -> Result<Self, ()> {
        let Self::Number(divisor) = other else {
            return Err(());
        };
        if divisor == 0.0 {
            return Err(());
        }
        self.scale(1.0 / divisor)
    }

    fn scale(self, factor: f32) -> Result<Self, ()> {
        if !factor.is_finite() {
            return Err(());
        }
        match self {
            Self::Number(value) => finite(value * factor).map(Self::Number),
            Self::LengthPercentage(value) => value.scale(factor).map(Self::LengthPercentage),
        }
    }
}

impl CalcLengthPercentage {
    fn scale(self, factor: f32) -> Result<Self, ()> {
        let value = Self {
            percentage: self.percentage * factor,
            px: self.px * factor,
            em: self.em * factor,
            rem: self.rem * factor,
        };
        value.is_finite().then_some(value).ok_or(())
    }

    fn is_finite(self) -> bool {
        self.percentage.is_finite()
            && self.px.is_finite()
            && self.em.is_finite()
            && self.rem.is_finite()
    }
}

fn finite(value: f32) -> Result<f32, ()> {
    value.is_finite().then_some(value).ok_or(())
}

fn parse_error<'i>(input: &Parser<'i, '_>) -> cssparser::ParseError<'i, ()> {
    input.new_custom_error(())
}

fn parse_one<'i, 't>(input: &mut Parser<'i, 't>) -> Result<Numeric, cssparser::ParseError<'i, ()>> {
    let location = input.current_source_location();
    match input.next()? {
        &Token::Number { value, .. } if value.is_finite() => Ok(Numeric::Number(value)),
        &Token::Dimension {
            value, ref unit, ..
        } if value.is_finite() => {
            let unit = match_ignore_ascii_case(unit).ok_or_else(|| parse_error(input))?;
            Ok(Numeric::LengthPercentage(length_term(value, unit)))
        },
        &Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(Numeric::LengthPercentage(CalcLengthPercentage {
                percentage: unit_value,
                ..CalcLengthPercentage::default()
            }))
        },
        &Token::ParenthesisBlock => input.parse_nested_block(parse_sum),
        Token::Function(name) if name.eq_ignore_ascii_case("calc") => {
            input.parse_nested_block(parse_sum)
        },
        token => Err(location.new_unexpected_token_error(token.clone())),
    }
}

fn match_ignore_ascii_case(unit: &str) -> Option<LengthUnit> {
    if unit.eq_ignore_ascii_case("px") {
        Some(LengthUnit::Px)
    } else if unit.eq_ignore_ascii_case("em") {
        Some(LengthUnit::Em)
    } else if unit.eq_ignore_ascii_case("rem") {
        Some(LengthUnit::Rem)
    } else if unit.eq_ignore_ascii_case("in") {
        Some(LengthUnit::In)
    } else if unit.eq_ignore_ascii_case("cm") {
        Some(LengthUnit::Cm)
    } else if unit.eq_ignore_ascii_case("mm") {
        Some(LengthUnit::Mm)
    } else if unit.eq_ignore_ascii_case("q") {
        Some(LengthUnit::Q)
    } else if unit.eq_ignore_ascii_case("pt") {
        Some(LengthUnit::Pt)
    } else if unit.eq_ignore_ascii_case("pc") {
        Some(LengthUnit::Pc)
    } else {
        None
    }
}

fn length_term(value: f32, unit: LengthUnit) -> CalcLengthPercentage {
    match unit {
        LengthUnit::Em => CalcLengthPercentage {
            em: value,
            ..CalcLengthPercentage::default()
        },
        LengthUnit::Rem => CalcLengthPercentage {
            rem: value,
            ..CalcLengthPercentage::default()
        },
        LengthUnit::Px => CalcLengthPercentage {
            px: value,
            ..CalcLengthPercentage::default()
        },
        LengthUnit::In
        | LengthUnit::Cm
        | LengthUnit::Mm
        | LengthUnit::Q
        | LengthUnit::Pt
        | LengthUnit::Pc => CalcLengthPercentage {
            px: unit.to_px(value, 0.0, 0.0),
            ..CalcLengthPercentage::default()
        },
    }
}

fn parse_product<'i, 't>(
    input: &mut Parser<'i, 't>,
) -> Result<Numeric, cssparser::ParseError<'i, ()>> {
    let mut product = parse_one(input)?;
    loop {
        let start = input.state();
        match input.next() {
            Ok(&Token::Delim('*')) => {
                product = product
                    .product(parse_one(input)?)
                    .map_err(|()| parse_error(input))?;
            },
            Ok(&Token::Delim('/')) => {
                product = product
                    .divide(parse_one(input)?)
                    .map_err(|()| parse_error(input))?;
            },
            _ => {
                input.reset(&start);
                break;
            },
        }
    }
    Ok(product)
}

fn parse_sum<'i, 't>(input: &mut Parser<'i, 't>) -> Result<Numeric, cssparser::ParseError<'i, ()>> {
    let mut sum = parse_product(input)?;
    loop {
        let start = input.state();
        match input.next_including_whitespace() {
            Ok(&Token::WhiteSpace(_)) => {
                if input.is_exhausted() {
                    break;
                }
                let subtract = match input.next()? {
                    Token::Delim('+') => false,
                    Token::Delim('-') => true,
                    _ => {
                        input.reset(&start);
                        break;
                    },
                };
                let mut right = parse_product(input)?;
                if subtract {
                    right = right.negate().map_err(|()| parse_error(input))?;
                }
                sum = sum.sum(right).map_err(|()| parse_error(input))?;
            },
            _ => {
                input.reset(&start);
                break;
            },
        }
    }
    Ok(sum)
}

pub(super) fn parse_length_percentage(source: &str) -> Result<CalcLengthPercentage, ParseError> {
    let mut input_buffer = ParserInput::new(source);
    let mut input = Parser::new(&mut input_buffer);
    let result = (|| {
        input.expect_function_matching("calc")?;
        let value = input.parse_nested_block(parse_sum)?;
        input.expect_exhausted()?;
        Ok::<_, cssparser::ParseError<'_, ()>>(value)
    })();
    match result {
        Ok(Numeric::LengthPercentage(value)) => Ok(value),
        Ok(Numeric::Number(_)) | Err(_) => Err(ParseError::expected(
            "a dimensionally valid calc() length-percentage",
        )),
    }
}
