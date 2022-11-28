/// This module contains types and a macro for defining ergonomic newtype
/// wrappers that ensure that the numeric type they wrap falls within specific
/// bounds.
use std::{
    fmt,
    ops::RangeInclusive,
};

#[derive(Debug, Clone)]
pub struct OutOfRange<T>(pub T, pub RangeInclusive<T>);

impl<T> fmt::Display for OutOfRange<T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "value out of range. got {}, expected ({}..={})",
            self.0,
            self.1.start(),
            self.1.end()
        )
    }
}

macro_rules! constrained_num {
    ($name:ident, $base:ty, $range:expr) => {
        #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name($base);

        impl $name {
            pub const fn clamp(value: $base) -> Self {
                const RANGE: RangeInclusive<$base> = $range;
                if value > *RANGE.end() {
                    Self(*RANGE.end())
                } else if value < *RANGE.start() {
                    Self(*RANGE.start())
                } else {
                    Self(value)
                }
            }

            pub const fn mask(value: $base) -> Self {
                const RANGE: RangeInclusive<$base> = $range;
                if value < *RANGE.start() {
                    Self(*RANGE.start())
                } else if value > *RANGE.end() {
                    Self(value & *RANGE.end())
                } else {
                    Self(value)
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self(*$range.start())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl Deref for $name {
            type Target = $base;
            fn deref(&self) -> &$base {
                &self.0
            }
        }

        impl TryFrom<$base> for $name {
            type Error = OutOfRange<$base>;
            fn try_from(other: $base) -> Result<Self, Self::Error> {
                const RANGE: RangeInclusive<$base> = $range;
                if RANGE.contains(&other) {
                    Ok(Self(other))
                } else {
                    Err(OutOfRange(other, RANGE))
                }
            }
        }
    };
}
