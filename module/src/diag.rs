//! Diagnostic snapshot file (RBDG). Pure format code: record encode/decode and a capped appender.
//! Record layout (little endian):
//!   header: magic "RBDG", u32 version=1, i64 qpc_100ns, u32 deck_count, u32 region_count
//!   region: u32 deck, u32 region_id, u64 address, u32 length, `length` bytes (length 0 = unreadable)

use crate::log;
use std::io::Write;

pub const MAGIC: &[u8; 4] = b"RBDG";
pub const VERSION: u32 = 1;
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;
pub const GLOBAL_DECK: u32 = 0xffff_ffff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub deck: u32,
    pub id: u32,
    pub addr: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Record {
    pub qpc_100ns: i64,
    pub deck_count: u32,
    pub regions: Vec<Region>,
}

pub fn encode(qpc_100ns: i64, deck_count: u32, regions: &[Region]) -> Vec<u8> {
    let mut v = Vec::with_capacity(24 + regions.iter().map(|r| 20 + r.bytes.len()).sum::<usize>());
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&VERSION.to_le_bytes());
    v.extend_from_slice(&qpc_100ns.to_le_bytes());
    v.extend_from_slice(&deck_count.to_le_bytes());
    v.extend_from_slice(&(regions.len() as u32).to_le_bytes());
    for r in regions {
        v.extend_from_slice(&r.deck.to_le_bytes());
        v.extend_from_slice(&r.id.to_le_bytes());
        v.extend_from_slice(&r.addr.to_le_bytes());
        v.extend_from_slice(&(r.bytes.len() as u32).to_le_bytes());
        v.extend_from_slice(&r.bytes);
    }
    v
}

#[cfg_attr(not(test), allow(dead_code))]
/// Decodes every complete record in `buf`; a truncated tail is ignored.
pub fn decode(buf: &[u8]) -> Vec<Record> {
    fn u32_at(b: &[u8], o: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    }
    fn u64_at(b: &[u8], o: usize) -> Option<u64> {
        Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
    }
    let mut out = Vec::new();
    let mut p = 0usize;
    loop {
        let rec = (|| {
            if buf.get(p..p + 4)? != MAGIC || u32_at(buf, p + 4)? != VERSION {
                return None;
            }
            let qpc = u64_at(buf, p + 8)? as i64;
            let deck_count = u32_at(buf, p + 16)?;
            let n = u32_at(buf, p + 20)? as usize;
            let mut q = p + 24;
            let mut regions = Vec::new();
            for _ in 0..n {
                let (deck, id, addr, len) = (u32_at(buf, q)?, u32_at(buf, q + 4)?, u64_at(buf, q + 8)?, u32_at(buf, q + 16)? as usize);
                let bytes = buf.get(q + 20..q + 20 + len)?.to_vec();
                regions.push(Region { deck, id, addr, bytes });
                q += 20 + len;
            }
            Some((Record { qpc_100ns: qpc, deck_count, regions }, q))
        })();
        match rec {
            Some((r, q)) => {
                out.push(r);
                p = q;
            }
            None => break,
        }
    }
    out
}

pub struct Writer {
    file: Option<std::fs::File>,
    written: u64,
    stopped: bool,
}

impl Writer {
    pub fn open() -> Writer {
        let path = log::dir().join("diag.bin");
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        match &file {
            Some(_) => log::line(&format!("diag: writing snapshots to {}", path.display())),
            None => log::line(&format!("diag: cannot open {}", path.display())),
        }
        let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Writer { file, written, stopped: false }
    }

    pub fn append(&mut self, rec: &[u8]) {
        if self.stopped {
            return;
        }
        if self.written + rec.len() as u64 > MAX_BYTES {
            self.stopped = true;
            log::line("diag: 200 MB cap reached, no more snapshots written");
            return;
        }
        if let Some(f) = self.file.as_mut() {
            if f.write_all(rec).is_ok() {
                self.written += rec.len() as u64;
            } else {
                self.stopped = true;
                log::line("diag: write failed, snapshots stopped");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trip() {
        let regions = vec![
            Region { deck: 0, id: 1, addr: 0x1234_5678_9abc, bytes: (0..=255u8).collect() },
            Region { deck: 1, id: 6, addr: 0, bytes: vec![] },
            Region { deck: GLOBAL_DECK, id: 100, addr: u64::MAX, bytes: vec![7; 33] },
        ];
        let mut buf = encode(-5, 2, &regions);
        buf.extend(encode(99, 2, &regions[..1]));
        let got = decode(&buf);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].qpc_100ns, got[0].deck_count), (-5, 2));
        assert_eq!(got[0].regions, regions);
        assert_eq!(got[1].regions, regions[..1].to_vec());
        assert_eq!(&buf[..4], b"RBDG");
        buf.truncate(buf.len() - 3);
        assert_eq!(decode(&buf).len(), 1);
    }
}
