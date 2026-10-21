#![no_std]
#![no_main]

use oxcore::fs::{DIRENT_SIZE, Dirent};
use ulib::*;

fn kind(k: u16) -> char {
    match k {
        abi::T_DIR => 'd',
        abi::T_DEV => 'c',
        _ => '-',
    }
}

fn ls(path: &str) -> SysResult<()> {
    let fd = open(path, abi::O_RDONLY)?;
    let st = fstat(fd)?;
    if st.kind != abi::T_DIR {
        println!("{} {:>4} {:>8} {}", kind(st.kind), st.ino, st.size, path);
        return close(fd);
    }
    let data = read_to_end(fd)?;
    let _ = close(fd);
    for raw in data.chunks_exact(DIRENT_SIZE) {
        let de = Dirent::decode(raw);
        if de.inum == 0 {
            continue;
        }
        let full = format!("{}/{}", path.trim_end_matches('/'), de.name());
        match stat(&full) {
            Ok(st) => println!("{} {:>4} {:>8} {}", kind(st.kind), st.ino, st.size, de.name()),
            Err(e) => println!("? {:>4} {:>8} {} ({})", de.inum, "", de.name(), errno::name(e)),
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    let paths: &[&str] = if args.len() > 1 { &args[1..] } else { &["."] };
    let mut status = 0;
    for p in paths {
        if let Err(e) = ls(p) {
            perror(p, e);
            status = 1;
        }
    }
    status
}
