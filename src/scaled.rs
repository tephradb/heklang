use std::cmp::Ordering;
use std::fmt;

pub const RATIO_SCALE: u8 = 6;

/// The widest scale a `Decimal` or a `Money` may declare.
///
/// The units are an `i64`, and `i64::MAX` has nineteen digits. At nineteen places every
/// digit is fractional, so the type cannot hold the value `1` and that literal is already
/// out of range; at twenty [`text`]'s own divisor overflows a `u64`. The parser refuses a
/// wider scale, so neither is reachable from a program.
pub const MAX_SCALE: u8 = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    HalfUp,
    HalfEven,
    Down,
}

impl fmt::Display for Rounding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Rounding::HalfUp => "HalfUp",
            Rounding::HalfEven => "HalfEven",
            Rounding::Down => "Down",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Overflow,
    DivisionByZero,
    Inexact,
}

pub fn add(lhs: i64, rhs: i64) -> Result<i64, Error> {
    lhs.checked_add(rhs).ok_or(Error::Overflow)
}

pub fn sub(lhs: i64, rhs: i64) -> Result<i64, Error> {
    lhs.checked_sub(rhs).ok_or(Error::Overflow)
}

pub fn mul(lhs: i64, rhs: i64) -> Result<i64, Error> {
    lhs.checked_mul(rhs).ok_or(Error::Overflow)
}

pub fn div(lhs: i64, rhs: i64) -> Result<i64, Error> {
    if rhs == 0 {
        return Err(Error::DivisionByZero);
    }
    lhs.checked_div(rhs).ok_or(Error::Overflow)
}

pub fn rem(lhs: i64, rhs: i64) -> Result<i64, Error> {
    if rhs == 0 {
        return Err(Error::DivisionByZero);
    }
    lhs.checked_rem(rhs).ok_or(Error::Overflow)
}

pub fn neg(units: i64) -> Result<i64, Error> {
    units.checked_neg().ok_or(Error::Overflow)
}

pub fn pow10(scale: u8) -> Result<i128, Error> {
    10i128.checked_pow(u32::from(scale)).ok_or(Error::Overflow)
}

pub fn rescale(units: i64, from: u8, to: u8) -> Result<i64, Error> {
    if to == from {
        return Ok(units);
    }
    if to > from {
        let widened = i128::from(units)
            .checked_mul(pow10(to - from)?)
            .ok_or(Error::Overflow)?;
        return fit(widened);
    }
    round_div(i128::from(units), pow10(from - to)?, Rounding::HalfUp)
}

pub fn mul_ratio(
    units: i64,
    factor_units: i64,
    factor_scale: u8,
    rounding: Rounding,
) -> Result<i64, Error> {
    round_div(
        product(units, factor_units)?,
        pow10(factor_scale)?,
        rounding,
    )
}

pub fn mul_ratio_exact(units: i64, factor_units: i64, factor_scale: u8) -> Result<i64, Error> {
    exact_div(product(units, factor_units)?, pow10(factor_scale)?)
}

pub fn div_round(units: i64, divisor: i64, rounding: Rounding) -> Result<i64, Error> {
    round_div(i128::from(units), i128::from(divisor), rounding)
}

pub fn div_exact(units: i64, divisor: i64) -> Result<i64, Error> {
    exact_div(i128::from(units), i128::from(divisor))
}

pub fn ratio(numerator: i64, denominator: i64, scale: u8) -> Result<i64, Error> {
    let numerator = i128::from(numerator)
        .checked_mul(pow10(scale)?)
        .ok_or(Error::Overflow)?;
    round_div(numerator, i128::from(denominator), Rounding::HalfUp)
}

fn product(units: i64, factor_units: i64) -> Result<i128, Error> {
    i128::from(units)
        .checked_mul(i128::from(factor_units))
        .ok_or(Error::Overflow)
}

fn round_div(numerator: i128, denominator: i128, rounding: Rounding) -> Result<i64, Error> {
    if denominator == 0 {
        return Err(Error::DivisionByZero);
    }

    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder == 0 {
        return fit(quotient);
    }

    let away = if (numerator < 0) != (denominator < 0) {
        -1
    } else {
        1
    };
    let twice = remainder.unsigned_abs() * 2;
    let magnitude = denominator.unsigned_abs();
    let adjust = match rounding {
        Rounding::Down => 0,
        Rounding::HalfUp => {
            if twice >= magnitude {
                away
            } else {
                0
            }
        }
        Rounding::HalfEven => match twice.cmp(&magnitude) {
            Ordering::Greater => away,
            Ordering::Less => 0,
            Ordering::Equal if quotient % 2 != 0 => away,
            Ordering::Equal => 0,
        },
    };

    fit(quotient.checked_add(adjust).ok_or(Error::Overflow)?)
}

fn exact_div(numerator: i128, denominator: i128) -> Result<i64, Error> {
    if denominator == 0 {
        return Err(Error::DivisionByZero);
    }
    if numerator % denominator != 0 {
        return Err(Error::Inexact);
    }
    fit(numerator / denominator)
}

fn fit(value: i128) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::Overflow)
}

pub fn write(f: &mut fmt::Formatter<'_>, units: i64, scale: u8) -> fmt::Result {
    f.write_str(&text(units, scale))
}

/// The same rendering as [`write`], as a string. Needed where a value is serialised
/// rather than displayed, which is how `Money` and `Decimal` reach JSON (rule 8).
pub fn text(units: i64, scale: u8) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    if scale == 0 {
        return format!("{sign}{abs}");
    }

    let divisor = 10u64.pow(u32::from(scale));
    let width = usize::from(scale);
    format!(
        "{sign}{}.{:0width$}",
        abs / divisor,
        abs % divisor,
        width = width
    )
}
