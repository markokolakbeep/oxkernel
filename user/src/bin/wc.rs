#![no_std]
#![no_main]

use ulib::*;

fn count(fd: usize) -> SysResult<(usize, usize, usize)> {
    let (mut lines, mut words, mut bytes) = (0, 0, 0);
    let mut in_word = false;
    let mut buf = [0u8; 4096];
    loop {
        let n = read(fd, &mut buf)?;
        if n == 0 {
            return Ok((lines, words, bytes));
        }
        bytes += n;
        for &c in &buf[..n] {
            if c == b'\n' {
                lines += 1;
            }
            if c.is_ascii_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                words += 1;
            }
        }
    }
}

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    let (fd, name) = match args.get(1) {
        Some(p) => match open(p, abi::O_RDONLY) {
            Ok(fd) => (fd, *p),
            Err(e) => {
                perror(p, e);
                return 1;
            }
        },
        None => (0, ""),
    };
    match count(fd) {
        Ok((l, w, b)) => {
            println!("{l:>7} {w:>7} {b:>7} {name}");
            0
        }
        Err(e) => {
            perror("wc", e);
            1
        }
    }
}
