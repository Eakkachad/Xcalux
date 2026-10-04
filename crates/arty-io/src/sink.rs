//! Append-only output used by the writer, and a fault-injecting wrapper
//! for crash tests.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

/// Where records are written. Writes only ever append; `set_len` drops a
/// torn tail before appending again.
pub trait Sink {
    fn append(&mut self, b: &[u8]) -> io::Result<()>;
    /// Make everything appended so far durable (a write barrier).
    fn sync(&mut self) -> io::Result<()>;
    fn set_len(&mut self, n: u64) -> io::Result<()>;
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A file opened for appending. Tracks its length so `len` needs no
/// system call, and keeps the cursor at the end.
pub struct FileSink {
    file: File,
    len: u64,
}

impl FileSink {
    /// Appends after the current end of `file`.
    pub fn new(mut file: File) -> io::Result<Self> {
        let len = file.seek(SeekFrom::End(0))?;
        Ok(Self { file, len })
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn into_file(self) -> File {
        self.file
    }
}

impl Sink for FileSink {
    fn append(&mut self, b: &[u8]) -> io::Result<()> {
        match self.file.write_all(b) {
            Ok(()) => {
                self.len += b.len() as u64;
                Ok(())
            }
            Err(e) => {
                // A partial write moved the cursor; keep `len` honest.
                if let Ok(pos) = self.file.stream_position() {
                    self.len = pos;
                }
                Err(e)
            }
        }
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    fn set_len(&mut self, n: u64) -> io::Result<()> {
        self.file.set_len(n)?;
        self.file.seek(SeekFrom::Start(n))?;
        self.len = n;
        Ok(())
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// In-memory sink (tests, and building small files before writing them).
/// `Vec`'s inherent `append`/`set_len` shadow these on a concrete `Vec`;
/// generic code over `S: Sink` is unaffected.
impl Sink for Vec<u8> {
    fn append(&mut self, b: &[u8]) -> io::Result<()> {
        self.extend_from_slice(b);
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn set_len(&mut self, n: u64) -> io::Result<()> {
        let n = usize::try_from(n).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        self.resize(n, 0);
        Ok(())
    }

    fn len(&self) -> u64 {
        Vec::len(self) as u64
    }
}

/// Simulates a crash after `k` more bytes: the append that crosses the
/// limit writes only the bytes before it (a torn write), and every call
/// after that fails. With `lose_unsynced`, the crash also drops whatever
/// was appended since the last `sync`, as a power cut would.
#[cfg(any(test, feature = "fault-injection"))]
pub struct FailAfter<S> {
    inner: S,
    budget: u64,
    lose_unsynced: bool,
    synced_len: u64,
    crashed: bool,
}

#[cfg(any(test, feature = "fault-injection"))]
impl<S: Sink> FailAfter<S> {
    pub fn new(inner: S, k: u64, lose_unsynced: bool) -> Self {
        let synced_len = inner.len();
        Self { inner, budget: k, lose_unsynced, synced_len, crashed: false }
    }

    pub fn crashed(&self) -> bool {
        self.crashed
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn into_inner(self) -> S {
        self.inner
    }

    fn dead() -> io::Error {
        io::Error::other("injected crash")
    }

    fn crash(&mut self) -> io::Error {
        self.crashed = true;
        if self.lose_unsynced {
            let _ = self.inner.set_len(self.synced_len);
        }
        Self::dead()
    }
}

#[cfg(any(test, feature = "fault-injection"))]
impl<S: Sink> Sink for FailAfter<S> {
    fn append(&mut self, b: &[u8]) -> io::Result<()> {
        if self.crashed {
            return Err(Self::dead());
        }
        let n = b.len().min(usize::try_from(self.budget).unwrap_or(usize::MAX));
        self.inner.append(&b[..n])?;
        self.budget -= n as u64;
        if n < b.len() {
            return Err(self.crash());
        }
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        if self.crashed {
            return Err(Self::dead());
        }
        self.inner.sync()?;
        self.synced_len = self.inner.len();
        Ok(())
    }

    /// Truncation is modelled as durable immediately.
    fn set_len(&mut self, n: u64) -> io::Result<()> {
        if self.crashed {
            return Err(Self::dead());
        }
        self.inner.set_len(n)?;
        self.synced_len = self.synced_len.min(n);
        Ok(())
    }

    fn len(&self) -> u64 {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_sink() {
        // Vec's own `append`/`set_len` shadow the trait's: call it explicitly.
        let mut v: Vec<u8> = Vec::new();
        Sink::append(&mut v, b"abc").unwrap();
        Sink::append(&mut v, b"de").unwrap();
        assert_eq!(Sink::len(&v), 5);
        Sink::set_len(&mut v, 2).unwrap();
        v.sync().unwrap();
        assert_eq!(v, b"ab");
        assert!(!Sink::is_empty(&v));
    }

    #[test]
    fn fail_after_tears_the_crossing_write() {
        let mut s = FailAfter::new(Vec::new(), 5, false);
        s.append(b"abc").unwrap();
        s.sync().unwrap();
        assert!(s.append(b"defg").is_err());
        assert!(s.crashed());
        assert!(s.append(b"").is_err() && s.sync().is_err() && s.set_len(0).is_err(), "dead after the crash");
        assert_eq!(s.into_inner(), b"abcde");

        // Exactly at the limit: the write completes, the next one fails.
        let mut s = FailAfter::new(Vec::new(), 3, false);
        s.append(b"abc").unwrap();
        s.append(b"").unwrap();
        assert!(s.append(b"d").is_err());
        assert_eq!(s.inner(), b"abc");
    }

    #[test]
    fn fail_after_can_lose_unsynced_bytes() {
        let mut s = FailAfter::new(b"hd".to_vec(), 6, true);
        s.append(b"ab").unwrap();
        s.sync().unwrap();
        s.append(b"cd").unwrap();
        assert!(s.append(b"efg").is_err());
        assert_eq!(s.into_inner(), b"hdab", "only synced bytes survive");

        let mut s = FailAfter::new(Vec::new(), 2, true);
        s.append(b"xyz").unwrap_err();
        assert!(s.inner().is_empty());

        let mut s = FailAfter::new(b"12345".to_vec(), 0, true);
        s.set_len(3).unwrap();
        s.append(b"x").unwrap_err();
        assert_eq!(s.into_inner(), b"123");
    }

    #[test]
    fn file_sink_appends_and_truncates() {
        let path = std::env::temp_dir().join(format!("arty-io-sink-{}.bin", std::process::id()));
        std::fs::write(&path, b"head").unwrap();
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let mut s = FileSink::new(file).unwrap();
        assert_eq!(s.len(), 4);
        s.append(b"-tail").unwrap();
        s.sync().unwrap();
        s.set_len(6).unwrap();
        s.append(b"!").unwrap();
        assert_eq!(s.len(), 7);
        drop(s);
        assert_eq!(std::fs::read(&path).unwrap(), b"head-t!");
        std::fs::remove_file(&path).unwrap();
    }
}
