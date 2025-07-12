// Remove when used.
#![allow(unused)]

mod codec;

use std::io;
#[cfg(unix)]
use std::sync::LazyLock;
#[cfg(windows)]
use std::sync::LazyLock;
use std::{
	future::Future,
	path::{Path, PathBuf},
	sync::Arc,
};

use bitcode::{DecodeOwned, Encode};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{FramedRead, FramedWrite};

pub trait Send<Req> {
	async fn send(&mut self, req: Req) -> io::Result<()>;
}

pub trait Recv<Res> {
	async fn recv(&mut self) -> io::Result<Option<Res>>;
}

pub struct Ipc<Req, Res, T> {
	reader: FramedRead<ReadHalf<T>, codec::Codec<Res>>,
	writer: FramedWrite<WriteHalf<T>, codec::Codec<Req>>,
}

impl<Req, Res, T> Ipc<Req, Res, T>
where
	Req: Encode + DecodeOwned,
	Res: Encode + DecodeOwned,
	T: AsyncRead + AsyncWrite,
{
	pub fn new(inner: T) -> Self {
		let (reader, writer) = tokio::io::split(inner);
		Self {
			reader: FramedRead::new(reader, codec::Codec::new()),
			writer: FramedWrite::new(writer, codec::Codec::new()),
		}
	}

	pub async fn close(&mut self) -> io::Result<()> {
		self.writer.close().await
	}
}

impl<Req, Res, T> Send<Req> for Ipc<Req, Res, T>
where
	Req: Encode,
	T: AsyncWrite,
{
	async fn send(&mut self, req: Req) -> io::Result<()> {
		self.writer.send(req).await
	}
}

impl<Req, Res, T> Send<Req> for Option<Ipc<Req, Res, T>>
where
	Req: Encode,
	T: AsyncWrite,
{
	async fn send(&mut self, req: Req) -> io::Result<()> {
		if let Some(this) = self.as_mut() {
			this.send(req).await
		} else {
			Ok(())
		}
	}
}

impl<Req, Res, T> Recv<Res> for Ipc<Req, Res, T>
where
	Res: DecodeOwned,
	T: AsyncRead,
{
	async fn recv(&mut self) -> io::Result<Option<Res>> {
		self.reader.next().await.transpose()
	}
}

impl<Req, Res, T> Recv<Res> for Option<Ipc<Req, Res, T>>
where
	Res: DecodeOwned,
	T: AsyncRead,
{
	async fn recv(&mut self) -> io::Result<Option<Res>> {
		if let Some(this) = self.as_mut() {
			this.recv().await
		} else {
			Ok(None)
		}
	}
}

#[cfg(unix)]
pub type Client<Req, Res> = self::Ipc<Req, Res, tokio::net::UnixStream>;
#[cfg(unix)]
pub type Server<Req, Res> = self::Ipc<Res, Req, tokio::net::UnixStream>;
#[cfg(unix)]
fn socket_path() -> PathBuf {
	let temp_dir = std::env::temp_dir();
	let _ = std::fs::create_dir_all(&temp_dir);
	temp_dir.join("gmodpatchtool.sock")
}

#[cfg(windows)]
pub type Client<Req, Res> = self::Ipc<Req, Res, tokio::net::windows::named_pipe::NamedPipeClient>;
#[cfg(windows)]
pub type Server<Req, Res> = self::Ipc<Req, Res, tokio::net::windows::named_pipe::NamedPipeServer>;
#[cfg(windows)]
const fn socket_path() -> &'static str {
	r"\\.\pipe\gmodpatchtool"
}

#[cfg(unix)]
pub async fn listen<Req, Res>() -> io::Result<Server<Req, Res>>
where
	Req: Encode + DecodeOwned,
	Res: Encode + DecodeOwned,
{
	let socket_path = socket_path();
	let listener = tokio::net::UnixListener::bind(&socket_path)?;
	let (stream, _) = listener.accept().await?;
	let _ = tokio::fs::remove_file(&socket_path).await;
	Ok(Server::new(stream))
}

#[cfg(unix)]
pub async fn connect<Req, Res>() -> io::Result<Client<Req, Res>>
where
	Req: Encode + DecodeOwned,
	Res: Encode + DecodeOwned,
{
	let socket_path = socket_path();
	while let Ok(false) = tokio::fs::try_exists(&socket_path).await {
		tokio::time::sleep(core::time::Duration::from_millis(50)).await;
	}

	Ok(Client::new(
		tokio::net::UnixStream::connect(&socket_path).await?,
	))
}

#[cfg(windows)]
pub async fn listen<Req, Res>() -> io::Result<Server<Req, Res>>
where
	Req: Encode + DecodeOwned,
	Res: Encode + DecodeOwned,
{
	let mut server = tokio::net::windows::named_pipe::ServerOptions::new()
		.first_pipe_instance(true)
		.create(socket_path())?;
	server.connect().await?;
	Ok(Server::new(server))
}

#[cfg(windows)]
pub async fn connect<Req, Res>() -> io::Result<Client<Req, Res>>
where
	Req: Encode + DecodeOwned,
	Res: Encode + DecodeOwned,
{
	use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

	let client = loop {
		match tokio::net::windows::named_pipe::ClientOptions::new().open(socket_path()) {
			Ok(client) => break client,
			Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {}
			Err(error) => return Err(error),
		}

		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	};
	Ok(Client::new(client))
}

#[cfg(test)]
mod tests {
	use bitcode::{Decode, Encode};

	use crate::ipc::{self, Recv as _, Send as _};

	#[derive(Encode, Decode)]
	enum Request {
		Ping(u32),
	}

	#[derive(Encode, Decode)]
	enum Response {
		Pong(u32),
	}

	async fn handler(req: Request) -> Option<Response> {
		match req {
			Request::Ping(count) => Some(Response::Pong(count)),
		}
	}

	#[cfg(any(windows, unix))]
	#[tokio::test]
	async fn example() {
		let join_handle = tokio::spawn(async move {
			let mut server = ipc::listen().await.unwrap();
			while let Ok(Some(result)) = server.recv().await {
				if let Some(response) = handler(result).await {
					if server.send(response).await.is_err() {
						break;
					}
				}
			}
		});

		let mut client = ipc::connect().await.unwrap();

		client.send(Request::Ping(42)).await.unwrap();
		client.close().await.unwrap();
		// This should now end because we closed the client side.
		join_handle.await.unwrap();
		let response = client.recv().await.unwrap();

		assert!(matches!(response, Some(Response::Pong(42))));
	}
}
