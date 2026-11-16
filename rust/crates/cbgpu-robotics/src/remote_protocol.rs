use crate::{Result, fail, require};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub yaw: f64,
}
impl Pose {
    pub fn validate(&self) -> Result<()> {
        require(
            [self.x, self.y, self.yaw]
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 100000.),
            "Invalid pose",
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    pub resolution: f64,
    pub origin: Pose,
    pub frame: String,
    pub session: String,
    pub version: u64,
    pub rle: Vec<i64>,
}
pub fn grid(value: &Value) -> Result<Grid> {
    let map: Grid =
        serde_json::from_value(value.clone()).map_err(|_| fail("Invalid map fields", 400))?;
    require(
        (1..=512).contains(&map.width) && (1..=512).contains(&map.height),
        "Map dimensions must be 1–512",
    )?;
    require(
        map.resolution.is_finite() && (0.005..=5.).contains(&map.resolution),
        "Invalid map resolution",
    )?;
    map.origin
        .validate()
        .map_err(|_| fail("Invalid map origin", 400))?;
    require(
        (1..=80).contains(&map.frame.chars().count()),
        "Invalid map frame",
    )?;
    require(
        (1..=80).contains(&map.session.chars().count()),
        "Invalid mapping session",
    )?;
    require(map.version >= 1, "Invalid map version")?;
    require(
        (2..=524288).contains(&map.rle.len()) && map.rle.len().is_multiple_of(2),
        "Invalid map RLE",
    )?;
    let mut total = 0u64;
    for run in map.rle.as_chunks::<2>().0 {
        require(
            (-1..=100).contains(&run[0]) && (1..=262144).contains(&run[1]),
            "Invalid map cell run",
        )?;
        total += run[1] as u64;
    }
    require(
        total == map.width as u64 * map.height as u64,
        "Map cells do not match dimensions",
    )?;
    Ok(map)
}
pub fn cell(map: &Grid, x: f64, y: f64) -> Result<i64> {
    require(x.is_finite() && y.is_finite(), "Coordinates must be finite")?;
    let dx = x - map.origin.x;
    let dy = y - map.origin.y;
    let (sin, cos) = map.origin.yaw.sin_cos();
    let col = ((cos * dx + sin * dy) / map.resolution).floor();
    let row = ((-sin * dx + cos * dy) / map.resolution).floor();
    require(
        col >= 0. && row >= 0. && col < (map.width as f64) && row < (map.height as f64),
        "Goal is outside the map",
    )?;
    let index = row as u64 * map.width as u64 + col as u64;
    let mut offset = 0;
    for run in map.rle.as_chunks::<2>().0 {
        offset += run[1] as u64;
        if index < offset {
            return Ok(run[0]);
        }
    }
    Err(fail("Invalid map index", 400))
}
pub fn free_goal(map: &Grid, x: f64, y: f64) -> Result<()> {
    require(
        cell(map, x, y)? == 0,
        "Choose an observed free cell, not an obstacle or unknown area",
    )
}
pub fn pose(value: &Value) -> Result<Pose> {
    let pose: Pose =
        serde_json::from_value(value.clone()).map_err(|_| fail("Invalid pose", 400))?;
    pose.validate()?;
    Ok(pose)
}
pub fn telemetry(value: &Value) -> Result<Value> {
    let mut value = value.clone();
    require(value.is_object(), "Telemetry must be an object")?;
    crate::protocol::bounded(&value, 16000, "Telemetry")?;
    for flag in ["armed", "ready", "emergency_stop", "navigation_enabled"] {
        require(
            value.get(flag).is_some_and(Value::is_boolean),
            "Telemetry state flags must be booleans",
        )?;
    }
    if let Some(position) = value.get("pose").filter(|value| !value.is_null()) {
        value["pose"] = serde_json::to_value(pose(position)?).expect("finite pose");
    }
    let age = value
        .get("sensor_age_s")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.)
        .ok_or_else(|| fail("Invalid sensor age", 400))?;
    if age > 1. || value.get("pose").is_none_or(Value::is_null) {
        value["ready"] = json!(false);
    }
    require(
        value.get("map_session").is_none_or(Value::is_string)
            && value
                .get("map_version")
                .is_none_or(|v| v.is_i64() || v.is_u64()),
        "Invalid map telemetry",
    )?;
    Ok(value)
}
pub fn jpeg(value: &str) -> Result<()> {
    if value.len() > 350000 {
        return Err(fail("Image exceeds 256 KB", 413));
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| fail("Invalid JPEG preview", 400))?;
    require(
        bytes.len() <= 256000 && bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9]),
        "Invalid JPEG preview",
    )
}
