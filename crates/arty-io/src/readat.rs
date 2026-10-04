//! Positional reads, so parallel decode tasks never share a file cursor.

use std::fs::File;
use std::io;

/// Read-only random access to a file or an in-memory buffer.
pub trait ReadAt {
    /// Fill `buf` from `offset`. Fails with `UnexpectedEof` when the source
    /// ends first.
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;

    /// Current length in bytes.
    fn len(&self) -> io::Result<u64>;

    fn is_empty(&self) -> io::Result<bool> {
        self.len().map(|n| n == 0)
    }
}

impl ReadAt for [u8] {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let src = usize::try_from(offset)
            .ok()
            .and_then(|start| self.get(start..)?.get(..buf.len()))
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn len(&self) -> io::Result<u64> {
        Ok(<[u8]>::len(self) as u64)
    }
}

impl ReadAt for Vec<u8> {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.as_slice().read_exact_at(buf, offset)
    }

    fn len(&self) -> io::Result<u64> {
        Ok(Vec::len(self) as u64)
    }
}

impl ReadAt for File {
    #[cfg(windows)]
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        // `seek_read` may return short counts; it also moves the cursor,
        // which nothing here relies on.
        while !buf.is_empty() {
            match self.seek_read(buf, offset) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buf, offset)
    }

    #[cfg(not(any(windows, unix)))]
    fn read_exact_at(&self, _buf: &mut [u8], _offset: u64) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    fn len(&self) -> io::Result<u64> {
        self.metadata().map(|m| m.len())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn check(src: &(impl ReadAt + ?Sized)) {
        assert_eq!(src.len().unwrap(), 10);
        let mut buf = [0u8; 4];
        src.read_exact_at(&mut buf, 3).unwrap();
        assert_eq!(buf, [3, 4, 5, 6]);
        src.read_exact_at(&mut buf, 6).unwrap();
        assert_eq!(buf, [6, 7, 8, 9]);
        src.read_exact_at(&mut [], 10).unwrap();
        for at in [7, 10, 11] {
            let err = src.read_exact_at(&mut buf, at).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "offset {at}");
        }
    }

    #[test]
    fn slice_and_file_read_at() {
        let data: Vec<u8> = (0..10).collect();
        check(&data[..]);
        let err = data[..].read_exact_at(&mut [0; 1], u64::MAX).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        let path = std::env::temp_dir().join(format!("arty-io-readat-{}.bin", std::process::id()));
        File::create(&path).unwrap().write_all(&data).unwrap();
        let file = File::open(&path).unwrap();
        check(&file);
        drop(file);
        std::fs::remove_file(&path).unwrap();
    }
}
