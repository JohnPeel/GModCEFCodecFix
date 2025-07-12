use std::{io, marker::PhantomData};

use bitcode::{DecodeOwned, Encode};
use tokio_util::{
	bytes::{Buf, BytesMut},
	codec::{Decoder, Encoder},
};

pub struct Codec<M>(PhantomData<fn(M) -> M>);

impl<M> Codec<M> {
	pub const fn new() -> Self {
		Self(PhantomData)
	}
}

impl<M> Decoder for Codec<M>
where
	M: DecodeOwned,
{
	type Item = M;
	type Error = io::Error;

	fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
		if src.len() < 4 {
			return Ok(None);
		}

		let mut length_bytes = [0u8; 4];
		length_bytes.copy_from_slice(&src[..4]);
		let length = u32::from_le_bytes(length_bytes) as usize;

		if src.len() < 4 + length {
			src.reserve(4 + length - src.len());
			return Ok(None);
		}

		let result = bitcode::decode::<M>(&src[4..4 + length]);
		src.advance(4 + length);

		result
			.map(Some)
			.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
	}
}

impl<M> Encoder<M> for Codec<M>
where
	M: Encode,
{
	type Error = io::Error;

	fn encode(&mut self, item: M, dst: &mut BytesMut) -> Result<(), Self::Error> {
		let item = bitcode::encode(&item);
		let length = u32::to_le_bytes(item.len() as u32);

		dst.reserve(4 + item.len());

		dst.extend_from_slice(&length);
		dst.extend_from_slice(item.as_slice());

		Ok(())
	}
}
