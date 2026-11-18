//! Fixed read-only NVML inventory query. No shell, arbitrary argv or raw-output reporting.
use cbgpu_gpu_enrollment::{
    GpuHealth,
    commands::protocol::{DiagnosticResult, valid_result},
};
use std::{
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
pub const ARGS: [&str; 2] = [
    "--query-gpu=index,name,memory.total,memory.used,utilization.gpu,uuid",
    "--format=csv,noheader,nounits",
];
const STDOUT_LIMIT: u64 = 16_384;
const STDERR_LIMIT: u64 = 4096;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    Unavailable,
    Invalid,
    Timeout,
}
impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "NVIDIA_UNAVAILABLE",
            Self::Invalid => "INVALID_INVENTORY",
            Self::Timeout => "DIAGNOSTIC_TIMEOUT",
        }
    }
}
pub trait Runner: Send + Sync {
    fn query(&self) -> Result<Vec<GpuHealth>, Failure>;
}
pub struct Nvidia {
    binary: PathBuf,
    timeout: Duration,
}
impl Default for Nvidia {
    fn default() -> Self {
        Self {
            binary: cbgpu_gpu_enrollment::nvidia_path::trusted()
                .map(PathBuf::from)
                .unwrap_or_default(),
            timeout: Duration::from_secs(5),
        }
    }
}
struct Process {
    child: Child,
    reaped: bool,
}
impl Process {
    fn stop(&mut self) {
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop()
    }
}
fn read_pipe<T: Read + Send + 'static>(
    pipe: T,
    limit: u64,
) -> mpsc::Receiver<Result<Vec<u8>, Failure>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Unavailable)
            .and({
                if bytes.len() as u64 > limit {
                    Err(Failure::Invalid)
                } else {
                    Ok(bytes)
                }
            });
        let _ = tx.send(result);
    });
    rx
}
impl Runner for Nvidia {
    fn query(&self) -> Result<Vec<GpuHealth>, Failure> {
        let mut command = Command::new(&self.binary);
        command
            .args(ARGS)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let parent = std::process::id();
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    #[cfg(target_os = "linux")]
                    {
                        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        if libc::getppid() != parent as i32 {
                            return Err(std::io::Error::other("Agent exited"));
                        }
                    }
                    Ok(())
                });
            }
        }
        let mut process = Process {
            child: command.spawn().map_err(|_| Failure::Unavailable)?,
            reaped: false,
        };
        let output = read_pipe(
            process.child.stdout.take().ok_or(Failure::Unavailable)?,
            STDOUT_LIMIT,
        );
        let errors = read_pipe(
            process.child.stderr.take().ok_or(Failure::Unavailable)?,
            STDERR_LIMIT,
        );
        let (mut stdout, mut stderr, mut status) = (None, None, None);
        let deadline = Instant::now() + self.timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(Failure::Timeout);
            }
            if stdout.is_none() {
                match output.try_recv() {
                    Ok(value) => stdout = Some(value?),
                    Err(mpsc::TryRecvError::Disconnected) => return Err(Failure::Unavailable),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if stderr.is_none() {
                match errors.try_recv() {
                    Ok(value) => stderr = Some(value?),
                    Err(mpsc::TryRecvError::Disconnected) => return Err(Failure::Unavailable),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if status.is_none() {
                status = process.child.try_wait().map_err(|_| Failure::Unavailable)?;
                if status.is_some() {
                    process.reaped = true
                }
            }
            if let (Some(out), Some(_), Some(exit)) = (&stdout, &stderr, &status) {
                return if exit.success() {
                    parse(out)
                } else {
                    Err(Failure::Unavailable)
                };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
fn fields(line: &str) -> Result<Vec<String>, Failure> {
    let (mut fields, mut field, mut quoted, mut after) = (Vec::new(), String::new(), false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"')
                } else {
                    quoted = false;
                    after = true
                }
            } else {
                field.push(c)
            }
        } else if c == ',' {
            fields.push(field.trim().to_string());
            field.clear();
            after = false
        } else if after {
            if !c.is_ascii_whitespace() {
                return Err(Failure::Invalid);
            }
        } else if c == '"' {
            if !field.trim().is_empty() {
                return Err(Failure::Invalid);
            }
            field.clear();
            quoted = true
        } else {
            field.push(c)
        }
    }
    if quoted {
        return Err(Failure::Invalid);
    }
    fields.push(field.trim().to_string());
    if fields.len() != 6 {
        return Err(Failure::Invalid);
    }
    Ok(fields)
}
fn number<T: std::str::FromStr>(value: &str) -> Result<T, Failure> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Failure::Invalid);
    }
    value.parse().map_err(|_| Failure::Invalid)
}
pub fn parse(bytes: &[u8]) -> Result<Vec<GpuHealth>, Failure> {
    if bytes.len() as u64 > STDOUT_LIMIT {
        return Err(Failure::Invalid);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Failure::Invalid)?;
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return Err(Failure::Invalid);
    }
    let mut gpus = Vec::new();
    for line in text.lines() {
        if gpus.len() == 16 {
            return Err(Failure::Invalid);
        }
        let p = fields(line)?;
        gpus.push(GpuHealth {
            index: number(&p[0])?,
            name: p[1].clone(),
            memory_total_mib: number(&p[2])?,
            memory_used_mib: number(&p[3])?,
            utilization_percent: number(&p[4])?,
            hardware_uuid: Some(p[5].clone()),
        })
    }
    let result = DiagnosticResult {
        status: "SUCCEEDED".into(),
        gpus: gpus.clone(),
        error_code: None,
    };
    if !valid_result(&result)
        || gpus.iter().enumerate().any(|(i, g)| {
            gpus[..i].iter().any(|other| {
                other
                    .hardware_uuid
                    .as_ref()
                    .zip(g.hardware_uuid.as_ref())
                    .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
            })
        })
    {
        return Err(Failure::Invalid);
    }
    Ok(gpus)
}
pub fn result(runner: &dyn Runner) -> DiagnosticResult {
    match runner.query() {
        Ok(gpus) => DiagnosticResult {
            status: "SUCCEEDED".into(),
            gpus,
            error_code: None,
        },
        Err(error) => DiagnosticResult {
            status: "FAILED".into(),
            gpus: Vec::new(),
            error_code: Some(error.code().into()),
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const ROW: &str =
        "0, Synthetic GPU, 24576, 1024, 4, GPU-11111111-1111-4111-8111-111111111111\n";
    #[test]
    fn typed_csv_rejects_unbounded_invalid_duplicate_or_executable_content() {
        let gpu = parse(ROW.as_bytes()).unwrap();
        assert_eq!(gpu[0].memory_total_mib, 24576);
        assert_eq!(gpu[0].name, "Synthetic GPU");
        assert!(
            parse(
                ROW.replace("Synthetic GPU", "\"Synthetic, GPU\"")
                    .as_bytes()
            )
            .is_ok()
        );
        for value in [
            "".to_string(),
            ROW.replace("24576", "N/A"),
            ROW.replace(", 4,", ", 101,"),
            ROW.replace("1024", "999999"),
            ROW.replace("0, ", "-1, "),
            ROW.replace("Synthetic GPU", "bad\u{1b}[31m"),
            ROW.replace("Synthetic GPU", "\"unterminated"),
            format!("{ROW}{ROW}"),
            "x".repeat(16385),
        ] {
            assert_eq!(parse(value.as_bytes()).err(), Some(Failure::Invalid))
        }
    }
    #[test]
    fn errors_are_structured_not_stdout() {
        struct Fake;
        impl Runner for Fake {
            fn query(&self) -> Result<Vec<GpuHealth>, Failure> {
                Err(Failure::Timeout)
            }
        }
        let response = result(&Fake);
        assert_eq!(response.error_code.as_deref(), Some("DIAGNOSTIC_TIMEOUT"));
        assert!(response.gpus.is_empty());
        assert!(valid_result(&response));
        assert_eq!(ARGS.len(), 2);
        assert!(
            !ARGS
                .iter()
                .any(|arg| arg.contains("reset") || arg.contains("power-limit"))
        )
    }
}
