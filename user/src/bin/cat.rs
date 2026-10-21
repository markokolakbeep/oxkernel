#![no_std]
#![no_main]

use ulib::*;

fn copy(fd: usize) -> SysResult<()> {
    let mut buf = [0u8; 4096];
    loop {
        let n = read(fd, &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        write_all(1, &buf[..n])?;
    }
}

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    if args.len() < 2 {
        return if copy(0).is_ok() { 0 } else { 1 };
    }
    let mut status = 0;
    for path in &args[1..] {
        match open(path, abi::O_RDONLY) {
            Ok(fd) => {
                if let Err(e) = copy(fd) {
                    perror(path, e);
                    status = 1;
                }
                let _ = close(fd);
            }
            Err(e) => {
                perror(path, e);
                status = 1;
            }
        }
    }
    status
}
