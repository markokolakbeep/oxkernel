//! Fixed-capacity byte ring buffer, used for pipes.

pub struct RingBuf<const N: usize> {
    buf: [u8; N],
    /// Free-running read and write counters; `w - r` is the fill level.
    r: usize,
    w: usize,
}

impl<const N: usize> Default for RingBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RingBuf<N> {
    pub const fn new() -> Self {
        Self { buf: [0; N], r: 0, w: 0 }
    }

    pub fn len(&self) -> usize {
        self.w.wrapping_sub(self.r)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == N
    }

    /// Copies in as much of `src` as fits, returning how much that was.
    pub fn push_slice(&mut self, src: &[u8]) -> usize {
        let n = src.len().min(N - self.len());
        for &b in &src[..n] {
            self.buf[self.w % N] = b;
            self.w = self.w.wrapping_add(1);
        }
        n
    }

    /// Copies out up to `dst.len()` bytes, returning how many.
    pub fn pop_slice(&mut self, dst: &mut [u8]) -> usize {
        let n = dst.len().min(self.len());
        for d in &mut dst[..n] {
            *d = self.buf[self.r % N];
            self.r = self.r.wrapping_add(1);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_and_capacity() {
        let mut rb = RingBuf::<8>::new();
        assert!(rb.is_empty());
        assert_eq!(rb.push_slice(b"hello world"), 8);
        assert!(rb.is_full());
        let mut out = [0u8; 5];
        assert_eq!(rb.pop_slice(&mut out), 5);
        assert_eq!(&out, b"hello");
        assert_eq!(rb.push_slice(b"XYZ!"), 4);
        let mut rest = [0u8; 16];
        let n = rb.pop_slice(&mut rest);
        assert_eq!(&rest[..n], b" woXYZ!");
        assert!(rb.is_empty());
    }

    #[test]
    fn wraps_many_times() {
        let mut rb = RingBuf::<7>::new();
        let mut expect = 0u8;
        let mut next = 0u8;
        for round in 0..1000 {
            let chunk: Vec<u8> = (0..(round % 5 + 1))
                .map(|_| {
                    next = next.wrapping_add(1);
                    next
                })
                .collect();
            let pushed = rb.push_slice(&chunk);
            next = next.wrapping_sub((chunk.len() - pushed) as u8);
            let mut out = [0u8; 3];
            let n = rb.pop_slice(&mut out);
            for &b in &out[..n] {
                expect = expect.wrapping_add(1);
                assert_eq!(b, expect);
            }
        }
    }
}
