use std::io::{self, Read, Write};
use std::net::TcpStream;

use anyhow::{Context, Result, bail};

use crate::{config::AUTH_TOKEN, config::MAX_FRAME_SIZE};

const MAGIC: u32 = 0x5243_5631;
const VERSION: u16 = 1;

#[derive(Debug, Clone)]
pub struct ClientHello {
    pub client_name: String,
}

pub fn write_hello(stream: &mut TcpStream, hello: &ClientHello) -> Result<()> {
    stream.write_all(&MAGIC.to_be_bytes())?;
    stream.write_all(&VERSION.to_be_bytes())?;

    write_string(stream, AUTH_TOKEN)?;
    write_string(stream, &hello.client_name)?;
    stream.flush()?;
    Ok(())
}

pub fn read_and_validate_hello(stream: &mut TcpStream) -> Result<ClientHello> {
    let mut magic_buf = [0_u8; 4];
    stream.read_exact(&mut magic_buf)?;
    let magic = u32::from_be_bytes(magic_buf);
    if magic != MAGIC {
        bail!("invalid protocol magic");
    }

    let mut version_buf = [0_u8; 2];
    stream.read_exact(&mut version_buf)?;
    let version = u16::from_be_bytes(version_buf);
    if version != VERSION {
        bail!("unsupported protocol version: {version}");
    }

    let token = read_string(stream).context("failed to read token")?;
    if token != AUTH_TOKEN {
        bail!("authentication failed");
    }

    let client_name = read_string(stream).context("failed to read client name")?;
    Ok(ClientHello { client_name })
}

pub fn write_frame(stream: &mut TcpStream, jpeg: &[u8]) -> Result<()> {
    if jpeg.len() > MAX_FRAME_SIZE {
        bail!("frame too large: {}", jpeg.len());
    }
    let len = jpeg.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(jpeg)?;
    Ok(())
}

pub fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_buf = [0_u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_SIZE {
        bail!("invalid frame size: {len}");
    }

    let mut buf = vec![0_u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_string(stream: &mut TcpStream, s: &str) -> io::Result<()> {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "string too long"))?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(bytes)
}

fn read_string(stream: &mut TcpStream) -> io::Result<String> {
    let mut len_buf = [0_u8; 2];
    stream.read_exact(&mut len_buf)?;
    let len = u16::from_be_bytes(len_buf) as usize;

    let mut bytes = vec![0_u8; len];
    stream.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid utf8 string"))
}
