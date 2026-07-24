/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Dimensional parsing and reduction for CSS math expressions.
//!
//! Harvested and reshaped from Stylo's precedence and dimensional-arithmetic
//! model in `style/values/specified/calc.rs` at
//! `b157d925267fdd37b03f43e3387ab2f0909e57b0`. Livery keeps only the stable
//! length-percentage subset needed by its current property lane. The compact
//! retained program covers arithmetic, comparison, stepped, trigonometric, and
//! exponential functions while delaying environmental length bases.

use cssparser::{Parser, ParserInput, Token};

use super::length::{MathOperand, MathOperation, MathToken, RoundingStrategy};
use super::{
    CalcLengthPercentage, Length, LengthUnit, MathLengthPercentage, ParseError,
    RelativeLengthEnvironment,
};

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
                let mut value = CalcLengthPercentage {
                    percentage: left.percentage + right.percentage,
                    px: left.px + right.px,
                    em: left.em + right.em,
                    rem: left.rem + right.rem,
                    ..CalcLengthPercentage::default()
                };
                value.add_relative_terms(left)?;
                value.add_relative_terms(right)?;
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
        let mut value = Self {
            percentage: self.percentage * factor,
            px: self.px * factor,
            em: self.em * factor,
            rem: self.rem * factor,
            ..self
        };
        value.scale_relative(factor);
        value.is_finite().then_some(value).ok_or(())
    }

    fn is_finite(self) -> bool {
        self.percentage.is_finite()
            && self.px.is_finite()
            && self.em.is_finite()
            && self.rem.is_finite()
            && self.relative_is_finite()
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
    } else if unit.eq_ignore_ascii_case("vw") {
        Some(LengthUnit::Vw)
    } else if unit.eq_ignore_ascii_case("vh") {
        Some(LengthUnit::Vh)
    } else if unit.eq_ignore_ascii_case("vi") {
        Some(LengthUnit::Vi)
    } else if unit.eq_ignore_ascii_case("vb") {
        Some(LengthUnit::Vb)
    } else if unit.eq_ignore_ascii_case("vmin") {
        Some(LengthUnit::Vmin)
    } else if unit.eq_ignore_ascii_case("vmax") {
        Some(LengthUnit::Vmax)
    } else if unit.eq_ignore_ascii_case("svw") {
        Some(LengthUnit::Svw)
    } else if unit.eq_ignore_ascii_case("svh") {
        Some(LengthUnit::Svh)
    } else if unit.eq_ignore_ascii_case("svi") {
        Some(LengthUnit::Svi)
    } else if unit.eq_ignore_ascii_case("svb") {
        Some(LengthUnit::Svb)
    } else if unit.eq_ignore_ascii_case("svmin") {
        Some(LengthUnit::Svmin)
    } else if unit.eq_ignore_ascii_case("svmax") {
        Some(LengthUnit::Svmax)
    } else if unit.eq_ignore_ascii_case("lvw") {
        Some(LengthUnit::Lvw)
    } else if unit.eq_ignore_ascii_case("lvh") {
        Some(LengthUnit::Lvh)
    } else if unit.eq_ignore_ascii_case("lvi") {
        Some(LengthUnit::Lvi)
    } else if unit.eq_ignore_ascii_case("lvb") {
        Some(LengthUnit::Lvb)
    } else if unit.eq_ignore_ascii_case("lvmin") {
        Some(LengthUnit::Lvmin)
    } else if unit.eq_ignore_ascii_case("lvmax") {
        Some(LengthUnit::Lvmax)
    } else if unit.eq_ignore_ascii_case("dvw") {
        Some(LengthUnit::Dvw)
    } else if unit.eq_ignore_ascii_case("dvh") {
        Some(LengthUnit::Dvh)
    } else if unit.eq_ignore_ascii_case("dvi") {
        Some(LengthUnit::Dvi)
    } else if unit.eq_ignore_ascii_case("dvb") {
        Some(LengthUnit::Dvb)
    } else if unit.eq_ignore_ascii_case("dvmin") {
        Some(LengthUnit::Dvmin)
    } else if unit.eq_ignore_ascii_case("dvmax") {
        Some(LengthUnit::Dvmax)
    } else if unit.eq_ignore_ascii_case("cqw") {
        Some(LengthUnit::Cqw)
    } else if unit.eq_ignore_ascii_case("cqh") {
        Some(LengthUnit::Cqh)
    } else if unit.eq_ignore_ascii_case("cqi") {
        Some(LengthUnit::Cqi)
    } else if unit.eq_ignore_ascii_case("cqb") {
        Some(LengthUnit::Cqb)
    } else if unit.eq_ignore_ascii_case("cqmin") {
        Some(LengthUnit::Cqmin)
    } else if unit.eq_ignore_ascii_case("cqmax") {
        Some(LengthUnit::Cqmax)
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
        unit if unit.is_relative() => {
            let mut calc = CalcLengthPercentage::default();
            calc.add_relative(unit, value)
                .expect("one atomic length has one relative term");
            calc
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
        _ => unreachable!("all relative units are handled above"),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MathDimension {
    Number,
    Length,
    Angle,
    Strategy,
    None,
}

#[derive(Default)]
struct MathBuilder {
    leaves: Vec<MathOperand>,
    tokens: Vec<MathToken>,
}

impl MathBuilder {
    fn operand(&mut self, operand: MathOperand) -> Result<MathDimension, ()> {
        let dimension = match operand {
            MathOperand::Number(_) => MathDimension::Number,
            MathOperand::Length(_) | MathOperand::Percentage(_) => MathDimension::Length,
            MathOperand::Angle(_) => MathDimension::Angle,
            MathOperand::RoundingStrategy(_) => MathDimension::Strategy,
            MathOperand::None => MathDimension::None,
        };
        let index = if let Some(index) = self.leaves.iter().position(|value| *value == operand) {
            u8::try_from(index).map_err(|_| ())?
        } else {
            let index = u8::try_from(self.leaves.len()).map_err(|_| ())?;
            self.leaves.push(operand);
            index
        };
        self.tokens.push(MathToken::Operand(index));
        Ok(dimension)
    }

    fn operation(&mut self, operation: MathOperation) {
        self.tokens.push(MathToken::Operation(operation));
    }
}

fn angle_radians(value: f32, unit: &str) -> Option<f32> {
    if unit.eq_ignore_ascii_case("deg") {
        Some(value.to_radians())
    } else if unit.eq_ignore_ascii_case("grad") {
        Some(value * std::f32::consts::PI / 200.0)
    } else if unit.eq_ignore_ascii_case("rad") {
        Some(value)
    } else if unit.eq_ignore_ascii_case("turn") {
        Some(value * std::f32::consts::TAU)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
enum GeneralFunction {
    Calc,
    Min,
    Max,
    Clamp,
    Round,
    Mod,
    Rem,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Sqrt,
    Hypot,
    Log,
    Exp,
}

fn general_function(name: &str) -> Option<GeneralFunction> {
    [
        ("calc", GeneralFunction::Calc),
        ("min", GeneralFunction::Min),
        ("max", GeneralFunction::Max),
        ("clamp", GeneralFunction::Clamp),
        ("round", GeneralFunction::Round),
        ("mod", GeneralFunction::Mod),
        ("rem", GeneralFunction::Rem),
        ("sin", GeneralFunction::Sin),
        ("cos", GeneralFunction::Cos),
        ("tan", GeneralFunction::Tan),
        ("asin", GeneralFunction::Asin),
        ("acos", GeneralFunction::Acos),
        ("atan", GeneralFunction::Atan),
        ("atan2", GeneralFunction::Atan2),
        ("pow", GeneralFunction::Pow),
        ("sqrt", GeneralFunction::Sqrt),
        ("hypot", GeneralFunction::Hypot),
        ("log", GeneralFunction::Log),
        ("exp", GeneralFunction::Exp),
    ]
    .into_iter()
    .find_map(|(candidate, function)| name.eq_ignore_ascii_case(candidate).then_some(function))
}

fn parse_general_one<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let location = input.current_source_location();
    match input.next()? {
        &Token::Number { value, .. } if value.is_finite() => builder
            .operand(MathOperand::Number(value))
            .map_err(|()| parse_error(input)),
        &Token::Dimension {
            value, ref unit, ..
        } if value.is_finite() => {
            if let Some(unit) = match_ignore_ascii_case(unit) {
                builder
                    .operand(MathOperand::Length(Length { value, unit }))
                    .map_err(|()| parse_error(input))
            } else if let Some(radians) = angle_radians(value, unit) {
                builder
                    .operand(MathOperand::Angle(radians))
                    .map_err(|()| parse_error(input))
            } else {
                Err(parse_error(input))
            }
        },
        &Token::Percentage { unit_value, .. } if unit_value.is_finite() => builder
            .operand(MathOperand::Percentage(unit_value))
            .map_err(|()| parse_error(input)),
        Token::Ident(name) if name.eq_ignore_ascii_case("pi") => builder
            .operand(MathOperand::Number(std::f32::consts::PI))
            .map_err(|()| parse_error(input)),
        Token::Ident(name) if name.eq_ignore_ascii_case("e") => builder
            .operand(MathOperand::Number(std::f32::consts::E))
            .map_err(|()| parse_error(input)),
        Token::Ident(name) if name.eq_ignore_ascii_case("none") => builder
            .operand(MathOperand::None)
            .map_err(|()| parse_error(input)),
        &Token::ParenthesisBlock => {
            input.parse_nested_block(|nested| parse_general_sum(nested, builder))
        },
        Token::Function(name) => {
            let function = general_function(name).ok_or_else(|| parse_error(input))?;
            input.parse_nested_block(|nested| match function {
                GeneralFunction::Calc => parse_general_sum(nested, builder),
                GeneralFunction::Min => {
                    parse_comparison_arguments(nested, builder, MathOperation::Min)
                },
                GeneralFunction::Max => {
                    parse_comparison_arguments(nested, builder, MathOperation::Max)
                },
                GeneralFunction::Clamp => parse_clamp_arguments(nested, builder),
                GeneralFunction::Round => parse_round_arguments(nested, builder),
                GeneralFunction::Mod => {
                    parse_binary_same_dimension(nested, builder, MathOperation::Mod)
                },
                GeneralFunction::Rem => {
                    parse_binary_same_dimension(nested, builder, MathOperation::Rem)
                },
                GeneralFunction::Sin => parse_trig_argument(nested, builder, MathOperation::Sin),
                GeneralFunction::Cos => parse_trig_argument(nested, builder, MathOperation::Cos),
                GeneralFunction::Tan => parse_trig_argument(nested, builder, MathOperation::Tan),
                GeneralFunction::Asin => {
                    parse_inverse_trig_argument(nested, builder, MathOperation::Asin)
                },
                GeneralFunction::Acos => {
                    parse_inverse_trig_argument(nested, builder, MathOperation::Acos)
                },
                GeneralFunction::Atan => {
                    parse_inverse_trig_argument(nested, builder, MathOperation::Atan)
                },
                GeneralFunction::Atan2 => parse_atan2_arguments(nested, builder),
                GeneralFunction::Pow => parse_pow_arguments(nested, builder),
                GeneralFunction::Sqrt => parse_number_unary(nested, builder, MathOperation::Sqrt),
                GeneralFunction::Hypot => parse_hypot_arguments(nested, builder),
                GeneralFunction::Log => parse_log_arguments(nested, builder),
                GeneralFunction::Exp => parse_number_unary(nested, builder, MathOperation::Exp),
            })
        },
        token => Err(location.new_unexpected_token_error(token.clone())),
    }
}

fn parse_general_product<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let mut dimension = parse_general_one(input, builder)?;
    loop {
        let start = input.state();
        let operation = match input.next() {
            Ok(&Token::Delim('*')) => MathOperation::Multiply,
            Ok(&Token::Delim('/')) => MathOperation::Divide,
            _ => {
                input.reset(&start);
                break;
            },
        };
        let right = parse_general_one(input, builder)?;
        if operation == MathOperation::Divide
            && matches!(builder.tokens.last(), Some(MathToken::Operand(index))
                if matches!(builder.leaves[usize::from(*index)], MathOperand::Number(0.0)))
        {
            return Err(parse_error(input));
        }
        dimension = match (operation, dimension, right) {
            (MathOperation::Multiply, MathDimension::Number, value)
            | (MathOperation::Multiply, value, MathDimension::Number)
                if value != MathDimension::None =>
            {
                value
            },
            (MathOperation::Divide, MathDimension::Number, MathDimension::Number) => {
                MathDimension::Number
            },
            (MathOperation::Divide, MathDimension::Length, MathDimension::Number) => {
                MathDimension::Length
            },
            (MathOperation::Divide, MathDimension::Length, MathDimension::Length) => {
                MathDimension::Number
            },
            (MathOperation::Divide, MathDimension::Angle, MathDimension::Number) => {
                MathDimension::Angle
            },
            (MathOperation::Divide, MathDimension::Angle, MathDimension::Angle) => {
                MathDimension::Number
            },
            _ => return Err(parse_error(input)),
        };
        builder.operation(operation);
    }
    Ok(dimension)
}

fn parse_general_sum<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let dimension = parse_general_product(input, builder)?;
    loop {
        let start = input.state();
        match input.next_including_whitespace() {
            Ok(&Token::WhiteSpace(_)) => {
                if input.is_exhausted() {
                    break;
                }
                let operation = match input.next()? {
                    Token::Delim('+') => MathOperation::Add,
                    Token::Delim('-') => MathOperation::Subtract,
                    _ => {
                        input.reset(&start);
                        break;
                    },
                };
                let right = parse_general_product(input, builder)?;
                if dimension == MathDimension::None || right != dimension {
                    return Err(parse_error(input));
                }
                builder.operation(operation);
            },
            _ => {
                input.reset(&start);
                break;
            },
        }
    }
    Ok(dimension)
}

fn parse_comparison_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
    operation: MathOperation,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let dimension = parse_general_sum(input, builder)?;
    if dimension == MathDimension::None {
        return Err(parse_error(input));
    }
    let mut count = 1;
    while !input.is_exhausted() {
        input.expect_comma()?;
        let right = parse_general_sum(input, builder)?;
        if right != dimension {
            return Err(parse_error(input));
        }
        builder.operation(operation);
        count += 1;
    }
    if count < 1 {
        return Err(parse_error(input));
    }
    Ok(dimension)
}

fn parse_clamp_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let minimum = parse_general_sum(input, builder)?;
    input.expect_comma()?;
    let preferred = parse_general_sum(input, builder)?;
    input.expect_comma()?;
    let maximum = parse_general_sum(input, builder)?;
    input.expect_exhausted()?;
    if preferred == MathDimension::None
        || (minimum != MathDimension::None && minimum != preferred)
        || (maximum != MathDimension::None && maximum != preferred)
    {
        return Err(parse_error(input));
    }
    builder.operation(MathOperation::Clamp);
    Ok(preferred)
}

fn rounding_strategy(name: &str) -> Option<RoundingStrategy> {
    if name.eq_ignore_ascii_case("nearest") {
        Some(RoundingStrategy::Nearest)
    } else if name.eq_ignore_ascii_case("up") {
        Some(RoundingStrategy::Up)
    } else if name.eq_ignore_ascii_case("down") {
        Some(RoundingStrategy::Down)
    } else if name.eq_ignore_ascii_case("to-zero") {
        Some(RoundingStrategy::ToZero)
    } else {
        None
    }
}

fn parse_round_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let strategy = input
        .try_parse(|nested| {
            let name = nested.expect_ident_cloned()?;
            let strategy = rounding_strategy(&name).ok_or_else(|| parse_error(nested))?;
            nested.expect_comma()?;
            Ok::<RoundingStrategy, cssparser::ParseError<'_, ()>>(strategy)
        })
        .unwrap_or(RoundingStrategy::Nearest);
    builder
        .operand(MathOperand::RoundingStrategy(strategy))
        .map_err(|()| parse_error(input))?;
    let value = parse_general_sum(input, builder)?;
    let step = if input.is_exhausted() {
        builder
            .operand(MathOperand::Number(1.0))
            .map_err(|()| parse_error(input))?
    } else {
        input.expect_comma()?;
        parse_general_sum(input, builder)?
    };
    input.expect_exhausted()?;
    if value == MathDimension::None || value == MathDimension::Strategy || step != value {
        return Err(parse_error(input));
    }
    builder.operation(MathOperation::Round);
    Ok(value)
}

fn parse_binary_same_dimension<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
    operation: MathOperation,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let left = parse_general_sum(input, builder)?;
    input.expect_comma()?;
    let right = parse_general_sum(input, builder)?;
    input.expect_exhausted()?;
    if left == MathDimension::None || left == MathDimension::Strategy || right != left {
        return Err(parse_error(input));
    }
    builder.operation(operation);
    Ok(left)
}

fn parse_trig_argument<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
    operation: MathOperation,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let dimension = parse_general_sum(input, builder)?;
    input.expect_exhausted()?;
    if !matches!(dimension, MathDimension::Number | MathDimension::Angle) {
        return Err(parse_error(input));
    }
    builder.operation(operation);
    Ok(MathDimension::Number)
}

fn parse_inverse_trig_argument<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
    operation: MathOperation,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let dimension = parse_general_sum(input, builder)?;
    input.expect_exhausted()?;
    if dimension != MathDimension::Number {
        return Err(parse_error(input));
    }
    builder.operation(operation);
    Ok(MathDimension::Angle)
}

fn parse_atan2_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    parse_binary_same_dimension(input, builder, MathOperation::Atan2)?;
    Ok(MathDimension::Angle)
}

fn parse_number_unary<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
    operation: MathOperation,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let dimension = parse_general_sum(input, builder)?;
    input.expect_exhausted()?;
    if dimension != MathDimension::Number {
        return Err(parse_error(input));
    }
    builder.operation(operation);
    Ok(MathDimension::Number)
}

fn parse_pow_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let left = parse_general_sum(input, builder)?;
    input.expect_comma()?;
    let right = parse_general_sum(input, builder)?;
    input.expect_exhausted()?;
    if left != MathDimension::Number || right != MathDimension::Number {
        return Err(parse_error(input));
    }
    builder.operation(MathOperation::Pow);
    Ok(MathDimension::Number)
}

fn parse_hypot_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let dimension = parse_general_sum(input, builder)?;
    if matches!(dimension, MathDimension::None | MathDimension::Strategy) {
        return Err(parse_error(input));
    }
    if input.is_exhausted() {
        builder.operation(MathOperation::Abs);
        return Ok(dimension);
    }
    while !input.is_exhausted() {
        input.expect_comma()?;
        let right = parse_general_sum(input, builder)?;
        if right != dimension {
            return Err(parse_error(input));
        }
        builder.operation(MathOperation::Hypot);
    }
    Ok(dimension)
}

fn parse_log_arguments<'i, 't>(
    input: &mut Parser<'i, 't>,
    builder: &mut MathBuilder,
) -> Result<MathDimension, cssparser::ParseError<'i, ()>> {
    let value = parse_general_sum(input, builder)?;
    if value != MathDimension::Number {
        return Err(parse_error(input));
    }
    if input.is_exhausted() {
        builder.operation(MathOperation::Ln);
    } else {
        input.expect_comma()?;
        let base = parse_general_sum(input, builder)?;
        input.expect_exhausted()?;
        if base != MathDimension::Number {
            return Err(parse_error(input));
        }
        builder.operation(MathOperation::Log);
    }
    Ok(MathDimension::Number)
}

pub(super) fn parse_math(source: &str) -> Result<MathLengthPercentage, ParseError> {
    parse_math_dimension(source, MathDimension::Length)
}

pub(super) fn parse_number(source: &str) -> Result<f32, ParseError> {
    resolve_constant_math(parse_math_dimension(source, MathDimension::Number)?)
        .ok_or_else(|| ParseError::expected("a finite bounded CSS number expression"))
}

pub(super) fn parse_angle(source: &str) -> Result<f32, ParseError> {
    resolve_constant_math(parse_math_dimension(source, MathDimension::Angle)?)
        .ok_or_else(|| ParseError::expected("a finite bounded CSS angle expression"))
}

fn resolve_constant_math(value: MathLengthPercentage) -> Option<f32> {
    value
        .resolve_relative(RelativeLengthEnvironment::uniform_viewport(100.0, 100.0))
        .resolve_font_relative(16.0, 16.0)
        .resolved_px()
}

fn parse_math_dimension(
    source: &str,
    expected: MathDimension,
) -> Result<MathLengthPercentage, ParseError> {
    let mut input_buffer = ParserInput::new(source);
    let mut input = Parser::new(&mut input_buffer);
    let mut builder = MathBuilder::default();
    let result = (|| {
        let value = parse_general_one(&mut input, &mut builder)?;
        input.expect_exhausted()?;
        if value != expected {
            return Err(parse_error(&input));
        }
        Ok::<_, cssparser::ParseError<'_, ()>>(value)
    })();
    result.map_err(|_| ParseError::expected("a bounded CSS math length-percentage expression"))?;
    MathLengthPercentage::new(&builder.leaves, &builder.tokens)
}
