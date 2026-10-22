#![macro_use]
#![allow(unused)]

use core::fmt::{Display, LowerHex};

// Host test builds unify the dev-dependency's `log` feature into a build
// that may also enable `defmt`; `log` takes precedence so both can coexist.

#[cfg(all(feature = "_no_usb", feature = "usb_log"))]
compile_error!("You may not enable `usb_log` for MCUs that don't have USB, or when USB is disabled.");

#[collapse_debuginfo(yes)]
macro_rules! assert {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::assert!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::assert!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! assert_eq {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::assert_eq!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::assert_eq!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! assert_ne {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::assert_ne!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::assert_ne!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug_assert {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::debug_assert!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::debug_assert!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug_assert_eq {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::debug_assert_eq!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::debug_assert_eq!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug_assert_ne {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::debug_assert_ne!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::debug_assert_ne!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! todo {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::todo!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::todo!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! unreachable {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::unreachable!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::unreachable!($($x)*);
        }
    };
}

/// FIXME: Could not use defmt::panic here since it's not compatible with `bitfield`
#[collapse_debuginfo(yes)]
macro_rules! panic {
    ($($x:tt)*) => {
        {
            #[cfg(any(not(feature = "defmt"), feature = "log"))]
            ::core::panic!($($x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::core::panic!($($x)*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! trace {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::trace!($s $(, $x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::trace!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::debug!($s $(, $x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::debug!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! info {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::info!($s $(, $x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::info!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! warn {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::warn!($s $(, $x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::warn!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! error {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::error!($s $(, $x)*);
            #[cfg(all(feature = "defmt", not(feature = "log")))]
            ::defmt::error!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[cfg(all(feature = "defmt", not(feature = "log")))]
#[collapse_debuginfo(yes)]
macro_rules! unwrap {
    ($($x:tt)*) => {
        ::defmt::unwrap!($($x)*)
    };
}

#[cfg(any(not(feature = "defmt"), feature = "log"))]
#[collapse_debuginfo(yes)]
macro_rules! unwrap {
    ($arg:expr) => {
        match $crate::fmt::Try::into_result($arg) {
            ::core::result::Result::Ok(t) => t,
            ::core::result::Result::Err(e) => {
                ::core::panic!("unwrap of `{}` failed: {:?}", ::core::stringify!($arg), e);
            }
        }
    };
    ($arg:expr, $($msg:expr),+ $(,)? ) => {
        match $crate::fmt::Try::into_result($arg) {
            ::core::result::Result::Ok(t) => t,
            ::core::result::Result::Err(e) => {
                ::core::panic!("unwrap of `{}` failed: {}: {:?}", ::core::stringify!($arg), ::core::format_args!($($msg,)*), e);
            }
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct NoneError;

pub trait Try {
    type Ok;
    type Error;
    fn into_result(self) -> Result<Self::Ok, Self::Error>;
}

impl<T> Try for Option<T> {
    type Ok = T;
    type Error = NoneError;

    #[inline]
    fn into_result(self) -> Result<T, NoneError> {
        self.ok_or(NoneError)
    }
}

impl<T, E> Try for Result<T, E> {
    type Ok = T;
    type Error = E;

    #[inline]
    fn into_result(self) -> Self {
        self
    }
}

pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl core::fmt::Debug for Bytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#02x?}", self.0)
    }
}

impl Display for Bytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#02x?}", self.0)
    }
}

impl LowerHex for Bytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#02x?}", self.0)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for Bytes<'_> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{:02x}", self.0)
    }
}

// Define a `Debug` trait which implies both `core::fmt::Debug` and (if `defmt` is enabled): `defmt::Format`.
// It can be used to constrain something to a loggable type.
#[cfg(feature = "defmt")]
pub trait Debug: defmt::Format + core::fmt::Debug {}
#[cfg(feature = "defmt")]
impl<T> Debug for T where T: defmt::Format + core::fmt::Debug {}
#[cfg(not(feature = "defmt"))]
pub use core::fmt::Debug;
