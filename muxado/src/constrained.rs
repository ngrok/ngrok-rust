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
    (clamp; $name:ty, $base:ty, $range:expr) => {
        impl $name {
            /// Clamp the provided value to the valid range for this number
            /// type.
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
        }
    };
    (mask; $name:ty, $base:ty, $range:expr) => {
        impl $name {
            /// Mask the provided value using the maximum for this number type.
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
    };
    ($(#[$outer:meta])* $name:ident, $base:ty, $range:expr, $($t:tt),*) => {
        $(#[$outer])*
        ///
        /// This is a type-safe wrapper for a primitive number type that
        /// enforces range or bitmask constraints.
        #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name($base);

        #[allow(dead_code, missing_docs)]
        impl $name {
            pub const MIN: $base = *$range.start();
            pub const MAX: $base = *$range.end();
            pub const BITS: $base = <$base>::BITS - $range.end().leading_zeros();
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

        $(constrained_num!($t; $name, $base, $range);)*
    };
    ($(#[$outer:meta])* $name:ident, $base:ty, $range:expr) => {
        constrained_num!($(#[$outer])* $name, $base, $range, clamp, mask);
    };
}
