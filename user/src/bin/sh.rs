//! The shell: pipelines, < and > redirection, `;` to run commands in turn,
//! & for background jobs, and the built-ins cd and exit. ^C interrupts the
//! foreground job.

#![no_std]
#![no_main]

use ulib::*;

struct Command<'a> {
    argv: Vec<&'a str>,
    input: Option<&'a str>,
    output: Option<&'a str>,
}

struct Job<'a> {
    stages: Vec<Command<'a>>,
    background: bool,
}

fn tokenize(line: &str) -> Result<Vec<&str>, &'static str> {
    let mut tokens = Vec::new();
    let mut rest = line;
    loop {
        rest = rest.trim_start();
        let Some(c) = rest.chars().next() else { break };
        if "|<>&;".contains(c) {
            tokens.push(&rest[..1]);
            rest = &rest[1..];
        } else if c == '"' {
            let end = rest[1..].find('"').ok_or("unterminated quote")?;
            tokens.push(&rest[1..1 + end]);
            rest = &rest[end + 2..];
        } else {
            let end = rest.find(|c: char| c.is_whitespace() || "|<>&;\"".contains(c)).unwrap_or(rest.len());
            tokens.push(&rest[..end]);
            rest = &rest[end..];
        }
    }
    Ok(tokens)
}

/// Splits a line into `;`-separated jobs.
fn parse_line(line: &str) -> Result<Vec<Job<'_>>, &'static str> {
    let tokens = tokenize(line)?;
    let mut jobs = Vec::new();
    for group in tokens.split(|t| *t == ";") {
        if let Some(job) = parse(group)? {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

fn parse<'a>(tokens: &[&'a str]) -> Result<Option<Job<'a>>, &'static str> {
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut job = Job { stages: Vec::new(), background: false };
    let mut cmd = Command { argv: Vec::new(), input: None, output: None };
    let mut it = tokens.iter().copied().peekable();
    while let Some(t) = it.next() {
        match t {
            "|" => {
                if cmd.argv.is_empty() {
                    return Err("empty command in pipeline");
                }
                job.stages.push(core::mem::replace(&mut cmd, Command { argv: Vec::new(), input: None, output: None }));
            }
            "<" => cmd.input = Some(it.next().ok_or("missing file after <")?),
            ">" => cmd.output = Some(it.next().ok_or("missing file after >")?),
            "&" => {
                if it.peek().is_some() {
                    return Err("& must end the line");
                }
                job.background = true;
            }
            word => cmd.argv.push(word),
        }
    }
    if cmd.argv.is_empty() {
        return Err("missing command");
    }
    job.stages.push(cmd);
    Ok(Some(job))
}

fn resolve(cmd: &str) -> String {
    if cmd.contains('/') { cmd.to_string() } else { format!("/bin/{cmd}") }
}

/// Replaces stdin/stdout with the files named by the redirections.
fn redirect(cmd: &Command) {
    if let Some(path) = cmd.input {
        let _ = close(0);
        if let Err(e) = open(path, abi::O_RDONLY) {
            perror(path, e);
            exit(1);
        }
    }
    if let Some(path) = cmd.output {
        let _ = close(1);
        if let Err(e) = open(path, abi::O_WRONLY | abi::O_CREATE | abi::O_TRUNC) {
            perror(path, e);
            exit(1);
        }
    }
}

/// Runs inside the job leader: builds the pipeline and waits for it.
fn run_pipeline(stages: &[Command]) -> i32 {
    let n = stages.len();
    let mut prev_read: Option<usize> = None;
    let mut last_pid = -1;
    for (i, cmd) in stages.iter().enumerate() {
        let pipe_fds = if i + 1 < n {
            match pipe() {
                Ok(p) => Some(p),
                Err(e) => {
                    perror("sh: pipe", e);
                    return 1;
                }
            }
        } else {
            None
        };
        match fork() {
            Ok(0) => {
                if let Some(r) = prev_read {
                    let _ = close(0);
                    let _ = dup(r);
                    let _ = close(r);
                }
                if let Some((r, w)) = pipe_fds {
                    let _ = close(1);
                    let _ = dup(w);
                    let _ = close(w);
                    let _ = close(r);
                }
                redirect(cmd);
                let path = resolve(cmd.argv[0]);
                let e = exec(&path, &cmd.argv);
                perror(cmd.argv[0], e);
                exit(127);
            }
            Ok(pid) => last_pid = pid,
            Err(e) => {
                perror("sh: fork", e);
                return 1;
            }
        }
        if let Some(r) = prev_read.take() {
            let _ = close(r);
        }
        if let Some((r, w)) = pipe_fds {
            let _ = close(w);
            prev_read = Some(r);
        }
    }
    let mut status = 0;
    while let Ok((pid, code)) = wait() {
        if pid == last_pid {
            status = code;
        }
    }
    status
}

fn builtin(argv: &[&str]) -> bool {
    match argv[0] {
        "cd" => {
            let dir = argv.get(1).copied().unwrap_or("/");
            if let Err(e) = chdir(dir) {
                perror(dir, e);
            }
            true
        }
        "exit" => exit(argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(0)),
        _ => false,
    }
}

fn reap_background(fg: i32) -> Option<i32> {
    // Collect children until the foreground job (if any) is done.
    loop {
        match wait() {
            Ok((pid, code)) if pid == fg => return Some(code),
            Ok((pid, code)) => println!("[{pid}] done ({code})"),
            Err(_) => return None,
        }
    }
}

#[unsafe(no_mangle)]
fn main(_args: &[&str]) -> i32 {
    let mut last_status = 0;
    loop {
        print!("{}$ ", if last_status == 0 { "" } else { "!" });
        let Some(line) = read_line(0) else {
            println!();
            return 0;
        };
        let jobs = match parse_line(&line) {
            Ok(jobs) => jobs,
            Err(msg) => {
                eprintln!("sh: {msg}");
                continue;
            }
        };
        for job in jobs {
            last_status = run_job(&job, last_status);
        }
    }
}

/// Runs one job; returns the status to show in the prompt.
fn run_job(job: &Job, last_status: i32) -> i32 {
    if job.stages.len() == 1 && !job.background && builtin(&job.stages[0].argv) {
        return last_status;
    }
    let leader = match fork() {
        Ok(0) => {
            setpgid();
            exit(run_pipeline(&job.stages));
        }
        Ok(pid) => pid,
        Err(e) => {
            perror("sh: fork", e);
            return 1;
        }
    };
    if job.background {
        println!("[{leader}]");
        return 0;
    }
    setfg(leader);
    let status = reap_background(leader).unwrap_or(-1);
    setfg(0);
    status
}
