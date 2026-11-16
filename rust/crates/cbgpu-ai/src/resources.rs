//! Read-only reservation/GPU inspection. Never reset or stop a notebook/user process.
use cbgpu_ai_store::{Result, fail};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    pub index: u32,
    pub uuid: String,
    pub name: String,
    pub memory: u64,
    pub utilization: u32,
    pub reserved: bool,
    pub available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Process {
    pub uuid: String,
    pub pid: u32,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cards: Vec<Card>,
    pub processes: Vec<Process>,
}
#[async_trait::async_trait]
pub trait Resources: Send + Sync {
    async fn snapshot(&self) -> Result<Snapshot>;
    fn free_bytes(&self, path: &Path) -> Result<u64>;
}
#[derive(Clone)]
pub struct NativeResources {
    pub database_container: String,
    pub database_role: String,
    pub tool_dir: Option<PathBuf>,
}
impl Default for NativeResources {
    fn default() -> Self {
        Self {
            database_container: "cbgpu-supabase-db".into(),
            database_role: "postgres".into(),
            tool_dir: None,
        }
    }
}
impl NativeResources {
    pub(crate) fn tool(&self, name: &str) -> PathBuf {
        self.tool_dir
            .as_ref()
            .map(|p| p.join(name))
            .unwrap_or_else(|| PathBuf::from(name))
    }
}
pub(crate) async fn command(program: &Path, args: &[String], seconds: u64) -> Result<String> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(seconds),
        tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| fail("Resource check timed out", 503))?
    .map_err(|_| fail("Resource check could not start", 503))?;
    if !result.status.success() {
        return Err(fail("Resource check unavailable", 503));
    }
    String::from_utf8(result.stdout).map_err(|_| fail("Invalid resource response", 503))
}
fn normalized_csv(input: &str) -> String {
    let mut result = String::new();
    let mut quoted = false;
    let mut after_comma = false;
    for c in input.chars() {
        if c == '"' {
            quoted = !quoted;
        }
        if after_comma && !quoted && (c == ' ' || c == '\t') {
            continue;
        }
        result.push(c);
        after_comma = c == ',' && !quoted;
    }
    result
}
pub fn parse_devices(csv: &str) -> Result<Vec<Card>> {
    let normalized = normalized_csv(csv);
    let mut cards = vec![];
    for row in csv::ReaderBuilder::new()
        .has_headers(false)
        .trim(csv::Trim::All)
        .from_reader(normalized.as_bytes())
        .records()
    {
        let row = row.map_err(|_| fail("Invalid GPU CSV", 503))?;
        if row.len() != 5 {
            return Err(fail("GPU metadata unavailable", 503));
        }
        let card = Card {
            index: row[0].parse().map_err(|_| fail("Invalid GPU index", 503))?,
            uuid: row[1].into(),
            name: row[2].into(),
            memory: row[3]
                .parse()
                .map_err(|_| fail("GPU memory unavailable", 503))?,
            utilization: row[4]
                .parse()
                .map_err(|_| fail("GPU utilization unavailable", 503))?,
            reserved: false,
            available: false,
        };
        if !card.uuid.starts_with("GPU-")
            || cards
                .iter()
                .any(|c: &Card| c.uuid == card.uuid || c.index == card.index)
        {
            return Err(fail("GPU identity unavailable", 503));
        }
        cards.push(card);
    }
    if cards.is_empty() {
        return Err(fail("No GPU metadata available", 503));
    }
    Ok(cards)
}
pub fn parse_processes(csv: &str) -> Result<Vec<Process>> {
    let normalized = normalized_csv(csv);
    let mut pids = vec![];
    for row in csv::ReaderBuilder::new()
        .has_headers(false)
        .trim(csv::Trim::All)
        .from_reader(normalized.as_bytes())
        .records()
    {
        let row = row.map_err(|_| fail("Invalid GPU CSV", 503))?;
        if row.len() != 2 {
            return Err(fail("GPU process status unavailable", 503));
        }
        pids.push(Process {
            uuid: row[0].into(),
            pid: row[1]
                .parse()
                .map_err(|_| fail("GPU process status unavailable", 503))?,
        });
    }
    Ok(pids)
}
pub fn apply_reservations(
    mut cards: Vec<Card>,
    pids: &[Process],
    reservations: &serde_json::Value,
) -> Result<Vec<Card>> {
    let rows = reservations
        .as_array()
        .ok_or_else(|| fail("Reservation snapshot unavailable", 503))?;
    let mut blocked = std::collections::HashSet::new();
    for row in rows {
        let gpu = row["gpu"]
            .as_u64()
            .or_else(|| row["gpu"].as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| fail("Invalid reservation GPU", 503))?;
        blocked.insert(gpu);
    }
    for c in &mut cards {
        c.reserved = blocked.contains(&(c.index as u64 + 1));
        c.available = !c.reserved && c.memory < 100 && !pids.iter().any(|p| p.uuid == c.uuid);
    }
    Ok(cards)
}
#[async_trait::async_trait]
impl Resources for NativeResources {
    async fn snapshot(&self) -> Result<Snapshot> {
        const QUERY: &str = "SELECT coalesce(json_agg(json_build_object('gpu',gpu_id,'start',extract(epoch from start_time),'end',extract(epoch from end_time))),'[]'::json) FROM public.reservations WHERE status='approved' AND end_time>now() AND start_time<now()+interval '5 minutes'";
        let docker = self.tool("docker");
        let smi = self.tool("nvidia-smi");
        let reservation_args = [
            "exec",
            &self.database_container,
            "psql",
            "-X",
            "-U",
            &self.database_role,
            "-d",
            "postgres",
            "-At",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            QUERY,
        ]
        .map(str::to_owned);
        let device_args = [
            "--query-gpu=index,uuid,name,memory.used,utilization.gpu",
            "--format=csv,noheader,nounits",
        ]
        .map(str::to_owned);
        let process_args = [
            "--query-compute-apps=gpu_uuid,pid",
            "--format=csv,noheader,nounits",
        ]
        .map(str::to_owned);
        let (reservations, cards, pids) = tokio::try_join!(
            command(&docker, &reservation_args, 8),
            command(&smi, &device_args, 8),
            command(&smi, &process_args, 8)
        )?;
        let reservations: serde_json::Value = serde_json::from_str(reservations.trim())
            .map_err(|_| fail("Reservation snapshot unavailable", 503))?;
        let processes = parse_processes(&pids)?;
        Ok(Snapshot {
            cards: apply_reservations(parse_devices(&cards)?, &processes, &reservations)?,
            processes,
        })
    }
    fn free_bytes(&self, path: &Path) -> Result<u64> {
        disk_free(path)
    }
}
#[cfg(unix)]
pub fn disk_free(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| fail("Invalid storage path", 503))?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), s.as_mut_ptr()) } != 0 {
        return Err(fail("Storage status unavailable", 503));
    }
    let s = unsafe { s.assume_init() };
    Ok(s.f_bavail.saturating_mul(s.f_frsize))
}
#[cfg(not(unix))]
pub fn disk_free(_: &Path) -> Result<u64> {
    Err(fail("Storage safety check unavailable", 503))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_and_foreign_processes_always_win() {
        let cards=parse_devices("0, GPU-one, RTX 4090, 0, 0\n1, GPU-two, RTX 4090, 0, 0\n2, GPU-three, RTX 4090, 101, 0").unwrap();
        let pids = parse_processes("GPU-two, 123").unwrap();
        let cards = apply_reservations(cards, &pids, &serde_json::json!([{"gpu":1}])).unwrap();
        assert!(cards[0].reserved && !cards[0].available);
        assert!(!cards[1].available && !cards[2].available);
    }
    #[test]
    fn malformed_resource_data_is_not_idle() {
        assert!(parse_devices("N/A").is_err());
        assert!(parse_processes("GPU-one, N/A").is_err());
    }
}
