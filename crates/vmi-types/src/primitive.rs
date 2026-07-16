use crate::error::ScalarDecodeError;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ByteOrder {
    LittleEndian,
    BigEndian,
}

pub trait Scalar: Sized {
    const WIDTH: usize;

    fn from_le_bytes(bytes: &[u8]) -> Self;
    fn from_be_bytes(bytes: &[u8]) -> Self;
}

macro_rules! impl_scalar {
    ($ty:ty) => {
        impl Scalar for $ty {
            const WIDTH: usize = core::mem::size_of::<$ty>();

            fn from_le_bytes(bytes: &[u8]) -> Self {
                let mut buf = [0u8; core::mem::size_of::<$ty>()];
                buf.copy_from_slice(bytes);
                <$ty>::from_le_bytes(buf)
            }

            fn from_be_bytes(bytes: &[u8]) -> Self {
                let mut buf = [0u8; core::mem::size_of::<$ty>()];
                buf.copy_from_slice(bytes);
                <$ty>::from_be_bytes(buf)
            }
        }
    };
}

impl_scalar!(u8);
impl_scalar!(u16);
impl_scalar!(u32);
impl_scalar!(u64);
impl_scalar!(u128);
impl_scalar!(i8);
impl_scalar!(i16);
impl_scalar!(i32);
impl_scalar!(i64);
impl_scalar!(i128);

pub fn decode_scalar<T: Scalar>(bytes: &[u8], order: ByteOrder) -> Result<T, ScalarDecodeError> {
    if bytes.len() != T::WIDTH {
        return Err(ScalarDecodeError::WrongWidth {
            expected: T::WIDTH,
            actual: bytes.len(),
        });
    }

    Ok(match order {
        ByteOrder::LittleEndian => T::from_le_bytes(bytes),
        ByteOrder::BigEndian => T::from_be_bytes(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_scalar_with_explicit_endianness() -> Result<(), ScalarDecodeError> {
        let value = decode_scalar::<u16>(&[0x34, 0x12], ByteOrder::LittleEndian)?;
        assert_eq!(value, 0x1234);
        Ok(())
    }
}
