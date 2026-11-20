//! Deterministic replays of the browser action games; score is always derived here.
use crate::replay::Rng;
use serde_json::Value;

pub fn min_duration_ms(game: &str, actions: &[Value]) -> u64 {
    let ticks = actions
        .iter()
        .filter(|a| match game {
            "snake" => matches!(a.as_str(), Some("UP" | "DOWN" | "LEFT" | "RIGHT")),
            _ => a.as_str() == Some("tick"),
        })
        .count() as u64;
    match game {
        "snake" => ticks * 110,
        "flappy" => ticks * 1000 / 60,
        "tetris" => ticks * 400,
        _ => 0,
    }
}
pub fn replay(
    game: &str,
    seed: u32,
    _params: &Value,
    actions: &[Value],
) -> Result<i64, &'static str> {
    if seed == 0 || actions.is_empty() || actions.len() > 20000 {
        return Err("invalid action count");
    }
    match game {
        "snake" => snake(seed, actions),
        "flappy" => flappy(seed, actions),
        "tetris" => tetris(seed, actions),
        _ => Err("unknown game"),
    }
}
fn snake(seed: u32, actions: &[Value]) -> Result<i64, &'static str> {
    let mut rng = Rng(seed);
    let mut body = vec![(10i32, 10i32)];
    let mut dir = "RIGHT";
    let mut score = 0;
    let mut terminal = false;
    fn food(rng: &mut Rng, body: &[(i32, i32)]) -> (i32, i32) {
        loop {
            let p = (rng.index(20) as i32, rng.index(20) as i32);
            if !body.contains(&p) {
                return p;
            }
        }
    }
    let mut target = food(&mut rng, &body);
    for a in actions {
        if terminal {
            return Err("action after game over");
        };
        let d = a.as_str().ok_or("invalid action")?;
        if d == "stop" {
            terminal = true;
            continue;
        }
        if !matches!(d, "UP" | "DOWN" | "LEFT" | "RIGHT") {
            return Err("invalid direction");
        }
        if matches!(
            (dir, d),
            ("UP", "DOWN") | ("DOWN", "UP") | ("LEFT", "RIGHT") | ("RIGHT", "LEFT")
        ) {
            return Err("reverse direction");
        };
        dir = d;
        let (x, y) = body[0];
        let p = match d {
            "UP" => (x, y - 1),
            "DOWN" => (x, y + 1),
            "LEFT" => (x - 1, y),
            _ => (x + 1, y),
        };
        if p.0 < 0 || p.0 >= 20 || p.1 < 0 || p.1 >= 20 || body.contains(&p) {
            terminal = true;
            continue;
        }
        body.insert(0, p);
        if p == target {
            score += 1;
            if body.len() == 400 {
                terminal = true
            } else {
                target = food(&mut rng, &body)
            }
        } else {
            body.pop();
        }
    }
    if terminal {
        Ok(score)
    } else {
        Err("unfinished game")
    }
}
fn flappy(seed: u32, actions: &[Value]) -> Result<i64, &'static str> {
    let mut rng = Rng(seed);
    let (mut y, mut vy) = (240.0, 0.0);
    let mut pipes: Vec<(f64, f64, bool)> = vec![];
    let (mut frame, mut score) = (0, 0);
    let (mut started, mut terminal) = (false, false);
    let mut flapped = false;
    for a in actions {
        if terminal {
            return Err("action after game over");
        };
        match a.as_str() {
            Some("flap") => {
                if flapped {
                    return Err("duplicate flap in frame");
                };
                started = true;
                flapped = true;
                vy = -8.5
            }
            Some("tick") => {
                if !started {
                    return Err("tick before start");
                };
                flapped = false;
                vy += 0.45;
                y += vy;
                frame += 1;
                if frame % 90 == 0 {
                    pipes.push((370.0, 60.0 + rng.next() * 220.0, false))
                }
                for p in &mut pipes {
                    p.0 -= 2.8;
                    if !p.2 && p.0 + 52.0 < 70.0 {
                        p.2 = true;
                        score += 1
                    }
                }
                pipes.retain(|p| p.0 > -62.0);
                terminal = !(10.0..=424.0).contains(&y)
                    || pipes.iter().any(|p| {
                        86.0 > p.0 + 4.0
                            && 54.0 < p.0 + 48.0
                            && (y - 14.0 < p.1 || y + 14.0 > p.1 + 140.0)
                    });
            }
            _ => return Err("invalid action"),
        }
    }
    if terminal {
        Ok(score)
    } else {
        Err("unfinished game")
    }
}
#[derive(Clone)]
struct Piece {
    shape: Vec<Vec<u8>>,
    x: i32,
    y: i32,
}
fn piece(rng: &mut Rng) -> Piece {
    let shapes = [
        vec![vec![1, 1, 1, 1]],
        vec![vec![1, 1], vec![1, 1]],
        vec![vec![0, 1, 0], vec![1, 1, 1]],
        vec![vec![0, 1, 1], vec![1, 1, 0]],
        vec![vec![1, 1, 0], vec![0, 1, 1]],
        vec![vec![1, 0, 0], vec![1, 1, 1]],
        vec![vec![0, 0, 1], vec![1, 1, 1]],
    ];
    let shape = shapes[rng.index(7)].clone();
    let x = 5 - shape[0].len() as i32 / 2;
    Piece { shape, x, y: 0 }
}
fn valid(board: &[[bool; 10]], p: &Piece) -> bool {
    p.shape.iter().enumerate().all(|(r, row)| {
        row.iter().enumerate().all(|(c, &v)| {
            let (x, y) = (p.x + c as i32, p.y + r as i32);
            v == 0
                || ((0..10).contains(&x) && (0..20).contains(&y) && !board[y as usize][x as usize])
        })
    })
}
fn tetris(seed: u32, actions: &[Value]) -> Result<i64, &'static str> {
    let mut rng = Rng(seed);
    let mut board = vec![[false; 10]; 20];
    let mut p = piece(&mut rng);
    let mut next = piece(&mut rng);
    let (mut score, mut terminal) = (0, false);
    for a in actions {
        if terminal {
            return Err("action after game over");
        };
        let action = a.as_str().ok_or("invalid action")?;
        let mut q = p.clone();
        let mut lock = false;
        match action {
            "left" => q.x -= 1,
            "right" => q.x += 1,
            "rotate" => {
                q.shape = (0..p.shape[0].len())
                    .map(|c| p.shape.iter().rev().map(|row| row[c]).collect())
                    .collect()
            }
            "tick" | "down" => {
                q.y += 1;
                if !valid(&board, &q) {
                    q = p.clone();
                    lock = true
                }
            }
            "drop" => {
                loop {
                    let mut below = q.clone();
                    below.y += 1;
                    if !valid(&board, &below) {
                        break;
                    };
                    q = below
                }
                lock = true
            }
            _ => return Err("invalid action"),
        }
        if valid(&board, &q) {
            p = q
        };
        if lock {
            for (r, row) in p.shape.iter().enumerate() {
                for (c, &v) in row.iter().enumerate() {
                    if v != 0 {
                        board[p.y as usize + r][p.x as usize + c] = true
                    }
                }
            }
            let before = board.len();
            board.retain(|row| !row.iter().all(|&v| v));
            let cleared = before - board.len();
            score += [0, 100, 300, 500, 800][cleared];
            while board.len() < 20 {
                board.insert(0, [false; 10])
            }
            p = next;
            next = piece(&mut rng);
            terminal = !valid(&board, &p);
        }
    }
    if terminal {
        Ok(score)
    } else {
        Err("unfinished game")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    // Golden traces generated by the browser JS physics with xorshift seed 1.
    #[test]
    fn browser_scoring_traces() {
        let mut snake_actions = vec![json!("UP"); 10];
        snake_actions.extend(vec![json!("LEFT"); 10]);
        snake_actions.push(json!("stop"));
        assert_eq!(replay("snake", 1, &json!({}), &snake_actions), Ok(1));
        let mut bird = vec![];
        for frame in 0..260 {
            if [0, 19, 54, 91, 128, 165, 202].contains(&frame) {
                bird.push(json!("flap"));
            }
            bird.push(json!("tick"));
        }
        assert_eq!(replay("flappy", 1, &json!({}), &bird), Ok(1));
        bird.push(json!("tick"));
        assert!(replay("flappy", 1, &json!({}), &bird).is_err());
    }
    #[test]
    fn snake_terminal_and_reject_after() {
        let mut a = vec![json!("RIGHT"); 10];
        assert_eq!(replay("snake", 1, &json!({}), &a), Ok(0));
        a.push(json!("stop"));
        assert!(replay("snake", 1, &json!({}), &a).is_err());
    }
    #[test]
    fn flappy_terminal() {
        let mut a = vec![json!("flap")];
        let mut accepted = None;
        for _ in 0..100 {
            a.push(json!("tick"));
            if let Ok(score) = replay("flappy", 1, &json!({}), &a) {
                accepted = Some(score);
                break;
            }
        }
        assert_eq!(accepted, Some(0));
        a.push(json!("flap"));
        assert!(replay("flappy", 1, &json!({}), &a).is_err());
    }
    #[test]
    fn tetris_drops_finish() {
        let mut a = vec![];
        let mut ended = false;
        for _ in 0..100 {
            a.push(json!("drop"));
            if replay("tetris", 1, &json!({}), &a).is_ok() {
                ended = true;
                break;
            }
        }
        assert!(ended);
        a.push(json!("left"));
        assert!(replay("tetris", 1, &json!({}), &a).is_err());
    }
    #[test]
    fn rejects_invalid_and_unfinished() {
        for game in ["snake", "flappy", "tetris"] {
            assert!(replay(game, 1, &json!({}), &[json!("cheat")]).is_err());
            assert!(replay(game, 1, &json!({}), &[]).is_err())
        }
        assert!(replay("snake", 1, &json!({}), &[json!("LEFT")]).is_err());
        assert_eq!(min_duration_ms("snake", &vec![json!("RIGHT"); 10]), 1100);
    }
}
