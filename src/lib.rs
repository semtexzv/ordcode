//! A set of primitives and [Serde](https://serde.rs) serializers for
//! fast, prefix-free encoding which preserves lexicographic ordering of values.
//!
//! It is intended for encoding keys and values in key-value databases.
//!
//! ## OMG! Yet another serialization format?
//!
//! In most existing designs, prefix-free encoding of byte sequences is performed by escaping
//! "end-of-sequence" bytes. This takes extra space, and makes it difficult to know sequence length
//! without processing the whole input buffer; this also complicates memory allocation for
//! deserialized data. Instead, we take advantage of the fact that exact record size is always
//! known in key-value databases. This implementation relies on "two-sided" buffer design:
//! sequence lengths are varint-encoded and pushed to the tail end of the buffer, so
//! it is possible to get original length of serialized byte sequence(s) by deserializing
//! a few bytes only.
//! For serialization, this implementation provides (very fast) calculation of exact size
//! of serialized data length before serialization itself. These features
//! enable effective and predictable buffer management for repetitive scans and no-heap
//! (`#[no-std]`) targets.
//!
//! ## Features
//!
//! * encodings in both ascending and descending lexicographic orderings are supported
//! * encoding puts lengths of variable-size sequences to the end of serialized data,
//!   so resulting encoding is prefix-free and friendly to lexicographic ordering
//! * zero allocations, supports `#[no_std]` environments
//! * method to cheaply get exact size of serialized data without doing actual serialization,
//!   for effective buffer management
//! * space-efficient varint encoding for sequence lengths and discriminants
//! * easily customizable (endianness, encoding of primitive types etc.), with useful pre-sets
//! * reader/writer traits for double-ended buffers, so you can implement your own or use
//!   implementations provided by the crate
//! * no unsafe code
//!
//! ## Cargo.toml features and dependencies
//!
//! * `serde` (on by default): include `serde` serializer and deserializer.
//!    If you need only primitives, you can opt out.
//! * `std` (on by default): opt out for `#[no-std]` use, you will lose some convenience methods
//!   which use `Vec<u8>`
//!
//! ## Stability guarantees
//! The underlying encoding format is simple and unlikely to change.
//! As a safeguard, `Serializer` and `Deserializer` implement `FormatVersion` trait for all serializer parameter
//! pre-sets (`params::AscendingOrder`, `params::PortableBinary`, `params::NativeBinary`).
//!
//! Note: serializing with descending lexicographic order is particularly useful for key-value
//! databases like _rocksdb_, where reverse iteration is slower than forward iteration.

#![crate_name = "ordcode"]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::map_err_ignore)]

#[cfg(feature = "serde")]
#[macro_use]
extern crate serde;

#[macro_use]
mod errors;

#[doc(inline)]
pub use errors::Error;

/// A convenient Result type
pub type Result<T = (), E = errors::Error> = core::result::Result<T, E>;

#[macro_use]
pub mod primitives;
pub mod bytes_esc;
pub mod varint;

pub mod buf;
pub mod params;

pub use buf::{DeBytesReader, DeBytesWriter, ReadFromTail, WriteToTail};
#[doc(inline)]
pub use params::Order;

#[cfg(feature = "serde")]
mod ord_de;
#[cfg(feature = "serde")]
mod ord_ser;
#[cfg(feature = "serde")]
mod size_calc;

#[doc(inline)]
#[cfg(feature = "serde")]
pub use ord_de::Deserializer;
#[doc(inline)]
#[cfg(feature = "serde")]
pub use ord_ser::Serializer;
#[doc(inline)]
#[cfg(feature = "serde")]
pub use size_calc::SizeCalc;

/// Current version of data encoding format for [`Serializer`] parametrized with
/// some [`params::SerializerParams`].
pub trait FormatVersion<P: params::SerializerParams> {
    const VERSION: u32;
}

/// Calculate exact size of serialized data for a [`serde::Serialize`] value.
///
/// Useful for calculating exact size of serialized objects for buffer allocations.
/// Calculation process is inexpensive, for fixed-size objects it evaluates to compile-time
/// constant, or a few `len()` method calls for variable-size objects.
///
/// ```
/// # use ordcode::{ calc_size, params };
/// # use serde::ser::Serialize;
///
/// #[derive(serde_derive::Serialize)]
/// struct Foo(u16, String);
/// let foo = Foo(1, "abc".to_string());
///
/// let data_size = calc_size(&foo, params::AscendingOrder).unwrap();
/// assert_eq!(data_size, 6);
/// ```
#[cfg(feature = "serde")]
pub fn calc_size<T, P>(value: &T, _params: P) -> Result<usize>
where
    T: ?Sized + serde::ser::Serialize,
    P: params::SerializerParams,
{
    let mut sc = size_calc::SizeCalc::<P>::new();
    value.serialize(&mut sc)?;
    Ok(sc.size())
}

/// Convenience method: same as [`calc_size()`], with [`params::AscendingOrder`]
#[cfg(feature = "serde")]
pub fn calc_size_asc<T>(value: &T) -> Result<usize>
where
    T: ?Sized + serde::ser::Serialize,
{
    calc_size(value, params::AscendingOrder)
}

/// Serialize `value` into pre-allocated byte buffer.
///
/// Buffer is supposed to be large enough to hold serialized data. You can use [`calc_size()`]
/// to get exact size of required buffer before serialization.
///
/// Returns the actual size of serialized data.
///
/// *Example*
/// ```
/// # use ordcode::{ Order, calc_size_asc, ser_to_buf_ordered };
/// # use serde::ser::Serialize;
///
/// #[derive(serde_derive::Serialize)]
/// struct Foo(u16, String);
/// let foo = Foo(1, "abc".to_string());
///
/// assert_eq!(calc_size_asc(&foo).unwrap(), 6);
/// let mut buf = [0_u8; 100];  // actually, need 6 bytes for this, but can use larger buffer
/// assert_eq!(ser_to_buf_ordered(&mut buf, &foo, Order::Ascending).unwrap(), 6);
/// assert_eq!(&buf[2..5], b"abc");
/// assert_eq!(buf[5], 7); // last byte is string length (3) in varint encoding
/// ```
#[cfg(feature = "serde")]
pub fn ser_to_buf_ordered<T>(buf: &mut [u8], value: &T, order: Order) -> Result<usize>
where
    T: ?Sized + serde::ser::Serialize,
{
    let mut de_buf = DeBytesWriter::new(buf);
    let mut ser = new_ser_asc(&mut de_buf);
    value.serialize(&mut ser)?;
    let len = de_buf.finalize()?;
    if matches!(order, Order::Descending) {
        primitives::invert_buffer(buf);
    }
    Ok(len)
}

/// Serialize `value` into pre-allocated, exact size byte buffer
///
/// Buffer is expected to be of exact size to hold serialized data. You can use [`calc_size()`]
/// to get exact size of required buffer before serialization.
///
/// In case of buffer underflow or buffer overflow, corresponding error is returned.
///
/// *Example*
/// ```
/// # use ordcode::{ Order, calc_size_asc, ser_to_buf_asc_exact };
/// # use serde::ser::Serialize;
///
/// #[derive(serde_derive::Serialize)]
/// struct Foo(u16, String);
/// let foo = Foo(1, "abc".to_string());
///
/// assert_eq!(calc_size_asc(&foo).unwrap(), 6);
/// let mut buf = [0_u8; 6];  // need buffer of exact size!
/// ser_to_buf_asc_exact(&mut buf, &foo).unwrap();
/// assert_eq!(&buf[2..5], b"abc");
/// assert_eq!(buf[5], 7); // last byte is string length (3) in varint encoding
/// ```
#[cfg(feature = "serde")]
pub fn ser_to_buf_asc_exact<T>(buf: &mut [u8], value: &T) -> Result
where
    T: ?Sized + serde::ser::Serialize,
{
    let mut de_buf = DeBytesWriter::new(buf);
    let mut ser = new_ser_asc(&mut de_buf);
    value.serialize(&mut ser)?;
    de_buf.is_complete()
}

/// Serialize `value` into byte vector
///
/// *Example*
/// ```
/// # use ordcode::{ Order, ser_to_vec_ordered };
/// # use serde::ser::Serialize;
///
/// #[derive(serde_derive::Serialize)]
/// struct Foo(u16, String);
/// let foo = Foo(1, "abc".to_string());
///
/// let buf = ser_to_vec_ordered(&foo, Order::Ascending).unwrap();
/// assert_eq!(&buf[2..5], b"abc");
/// assert_eq!(buf[5], 7); // last byte is string length (3) in varint encoding
/// ```
#[cfg(all(feature = "std", feature = "serde"))]
pub fn ser_to_vec_ordered<T>(value: &T, order: Order) -> Result<Vec<u8>>
where
    T: ?Sized + serde::ser::Serialize,
{
    let mut byte_buf = vec![0_u8; calc_size(value, params::AscendingOrder)?];
    let mut de_buf = DeBytesWriter::new(byte_buf.as_mut_slice());
    let mut ser = new_ser_asc(&mut de_buf);
    value.serialize(&mut ser)?;
    de_buf.is_complete()?;
    if matches!(order, Order::Descending) {
        primitives::invert_buffer(&mut byte_buf);
    }
    Ok(byte_buf)
}

/// Deserialize value from byte slice with [`params::AscendingOrder`]
///
/// *Example*
/// ```
/// # use serde::de::Deserialize;
/// # use ordcode::de_from_bytes_asc;
///
/// #[derive(serde_derive::Deserialize)]
/// struct Foo(u16, String);
///
/// let buf = [0_u8, 1, b'a', b'b', b'c', 7];
/// let foo: Foo = de_from_bytes_asc(&buf).unwrap();
/// assert_eq!(foo.0, 1);
/// assert_eq!(foo.1, "abc");
/// ```
#[cfg(feature = "serde")]
pub fn de_from_bytes_asc<'de, T>(input: &'de [u8]) -> Result<T>
where
    T: serde::de::Deserialize<'de>,
{
    let mut reader = DeBytesReader::new(input.as_ref());
    let mut deser = new_de_asc(&mut reader);
    T::deserialize(&mut deser)
}

/// Deserialize value from mutable byte slice.
///
/// For [`Order::Descending`], the buffer will be inverted in-place.
///
/// *Example*
/// ```
/// # use serde::de::Deserialize;
/// # use ordcode::{ Order, de_from_bytes_ordered, primitives };
///
/// #[derive(serde_derive::Deserialize)]
/// struct Foo(u16, String);
///
/// let mut buf = [255_u8, 254, 158, 157, 156, 248];
/// let foo: Foo = de_from_bytes_ordered(&mut buf, Order::Descending).unwrap();
/// assert_eq!(foo.0, 1);
/// assert_eq!(foo.1, "abc");
/// ```
#[cfg(feature = "serde")]
pub fn de_from_bytes_ordered<'de, T>(input: &'de mut [u8], order: Order) -> Result<T>
where
    T: serde::de::Deserialize<'de>,
{
    if matches!(order, Order::Descending) {
        primitives::invert_buffer(input.as_mut());
    }
    let mut reader = DeBytesReader::new(input.as_mut());
    let mut deser = new_de_asc(&mut reader);
    T::deserialize(&mut deser)
}

/// Create new default serializer instance (with [`params::AscendingOrder`])
#[cfg(feature = "serde")]
#[inline]
pub fn new_ser_asc<W>(writer: W) -> Serializer<W, params::AscendingOrder>
where
    W: buf::TailWriteBytes,
{
    Serializer::new(writer, params::AscendingOrder)
}

/// Create new default deserializer instance (with [`params::AscendingOrder`])
#[cfg(feature = "serde")]
#[inline]
pub fn new_de_asc<R>(reader: R) -> Deserializer<R, params::AscendingOrder>
where
    R: buf::TailReadBytes,
{
    Deserializer::new(reader, params::AscendingOrder)
}
