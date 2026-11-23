//! Minecraft Java Edition packet framing, VarInt, String, and Zlib compression codec.

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{self, Read, Write};

pub const MAX_PACKET_LEN: usize = 2 * 1024 * 1024;

pub fn read_varint<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut num = 0i32;
    let mut shift = 0u32;
    let mut buf = [0u8; 1];
    loop {
        r.read_exact(&mut buf)?;
        let b = buf[0];
        num |= i32::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Ok(num);
        }
        shift += 7;
        if shift >= 35 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "VarInt too big"));
        }
    }
}

pub fn write_varint<W: Write>(w: &mut W, value: i32) -> io::Result<()> {
    let mut v = value as u32;
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        w.write_all(&[byte])?;
        if v == 0 {
            return Ok(());
        }
    }
}

pub fn read_string(buf: &mut &[u8]) -> io::Result<String> {
    let len = read_varint(buf)?;
    if len < 0 || (len as usize) > buf.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid string length"));
    }
    let (head, rest) = buf.split_at(len as usize);
    *buf = rest;
    String::from_utf8(head.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8 string"))
}

pub fn write_string(out: &mut Vec<u8>, s: &str) -> io::Result<()> {
    write_varint(out, s.len() as i32)?;
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

pub fn read_u16_be(buf: &mut &[u8]) -> io::Result<u16> {
    if buf.len() < 2 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "u16 EOF"));
    }
    let val = u16::from_be_bytes([buf[0], buf[1]]);
    *buf = &buf[2..];
    Ok(val)
}

/// Reads one framed Minecraft packet `(packet_id, payload)` with optional Zlib compression.
pub fn read_packet<R: Read>(r: &mut R, compression_threshold: Option<i32>) -> io::Result<(i32, Vec<u8>)> {
    let packet_len = read_varint(r)?;
    if packet_len <= 0 || (packet_len as usize) > MAX_PACKET_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid packet length"));
    }
    let mut raw = vec![0u8; packet_len as usize];
    r.read_exact(&mut raw)?;

    let body: Vec<u8> = if compression_threshold.is_some() {
        let mut slice = raw.as_slice();
        let data_len = read_varint(&mut slice)?;
        if data_len == 0 {
            slice.to_vec()
        } else {
            let mut dec = ZlibDecoder::new(slice);
            let mut out = Vec::with_capacity((data_len as usize).min(MAX_PACKET_LEN));
            dec.read_to_end(&mut out)?;
            out
        }
    } else {
        raw
    };

    let mut slice = body.as_slice();
    let id = read_varint(&mut slice)?;
    Ok((id, slice.to_vec()))
}

/// Writes one framed Minecraft packet `(packet_id, payload)` with optional Zlib compression.
pub fn write_packet<W: Write>(
    w: &mut W,
    id: i32,
    payload: &[u8],
    compression_threshold: Option<i32>,
) -> io::Result<()> {
    let mut inner = Vec::with_capacity(5 + payload.len());
    write_varint(&mut inner, id)?;
    inner.extend_from_slice(payload);

    let framed = if let Some(threshold) = compression_threshold {
        let mut out = Vec::new();
        if (inner.len() as i32) >= threshold {
            write_varint(&mut out, inner.len() as i32)?;
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
            enc.write_all(&inner)?;
            out.extend_from_slice(&enc.finish()?);
        } else {
            write_varint(&mut out, 0)?;
            out.extend_from_slice(&inner);
        }
        out
    } else {
        inner
    };

    write_varint(w, framed.len() as i32)?;
    w.write_all(&framed)?;
    Ok(())
}
