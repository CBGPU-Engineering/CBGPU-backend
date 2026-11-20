use serde_json::Value;
pub struct Rng(pub u32);
impl Rng {
    pub fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        f64::from(x) / 4294967296.0
    }
    pub fn index(&mut self, n: usize) -> usize {
        (self.next() * n as f64) as usize
    }
}
pub fn replay(
    game: &str,
    seed: u32,
    params: &Value,
    actions: &[Value],
) -> Result<i64, &'static str> {
    if seed == 0 || actions.is_empty() || actions.len() > 20000 {
        return Err("invalid_actions");
    }
    let mut rng = Rng(seed);
    match game {
        "2048" => {
            let mut board = [0u32; 16];
            spawn(&mut board, &mut rng);
            spawn(&mut board, &mut rng);
            let mut score = 0i64;
            for a in actions {
                let dir = a.as_str().ok_or("invalid_direction")?;
                let before = board;
                for line in 0..4 {
                    let ids: Vec<usize> = match dir {
                        "left" => (0..4).map(|i| line * 4 + i).collect(),
                        "right" => (0..4).map(|i| line * 4 + 3 - i).collect(),
                        "up" => (0..4).map(|i| i * 4 + line).collect(),
                        "down" => (0..4).map(|i| (3 - i) * 4 + line).collect(),
                        _ => return Err("invalid_direction"),
                    };
                    let vals: Vec<u32> =
                        ids.iter().map(|&i| board[i]).filter(|&v| v != 0).collect();
                    let mut out = Vec::new();
                    let mut i = 0;
                    while i < vals.len() {
                        if i + 1 < vals.len() && vals[i] == vals[i + 1] {
                            let n = vals[i].checked_mul(2).ok_or("overflow")?;
                            out.push(n);
                            score += i64::from(n);
                            i += 2;
                        } else {
                            out.push(vals[i]);
                            i += 1;
                        }
                    }
                    for (j, &id) in ids.iter().enumerate() {
                        board[id] = *out.get(j).unwrap_or(&0);
                    }
                }
                if before == board {
                    return Err("unchanged_move");
                }
                spawn(&mut board, &mut rng);
            }
            Ok(score)
        }
        "memory" => {
            let pairs = params["pairs"].as_u64().unwrap_or(6) as usize;
            if ![6, 8, 12].contains(&pairs) {
                return Err("invalid_pairs");
            };
            let mut cards: Vec<usize> = (0..pairs).chain(0..pairs).collect();
            for i in (1..cards.len()).rev() {
                let j = rng.index(i + 1);
                cards.swap(i, j)
            }
            let mut matched = vec![false; cards.len()];
            let mut first = None;
            let mut moves = 0;
            for a in actions {
                if matched.iter().all(|&m| m) {
                    return Err("actions_after_completion");
                };
                let i = a.as_u64().ok_or("invalid_card")? as usize;
                if i >= cards.len() || matched[i] || first == Some(i) {
                    return Err("invalid_card");
                };
                if let Some(j) = first.take() {
                    moves += 1;
                    if cards[i] == cards[j] {
                        matched[i] = true;
                        matched[j] = true;
                    }
                } else {
                    first = Some(i)
                }
            }
            if !matched.iter().all(|&m| m) {
                return Err("incomplete_game");
            };
            Ok(1_000_000 - moves)
        }
        "mines" => {
            let (rows, cols, count) = match params["level"].as_str().unwrap_or("easy") {
                "easy" => (9, 9, 10),
                "medium" => (12, 12, 24),
                "hard" => (14, 16, 40),
                _ => return Err("invalid_level"),
            };
            let mut mines = vec![false; rows * cols];
            let mut seen = vec![false; rows * cols];
            let mut flags = vec![false; rows * cols];
            let mut placed = false;
            for a in actions {
                if seen.iter().filter(|&&v| v).count() == rows * cols - count {
                    return Err("actions_after_completion");
                };
                let r = a["r"].as_u64().ok_or("invalid_cell")? as usize;
                let c = a["c"].as_u64().ok_or("invalid_cell")? as usize;
                if r >= rows || c >= cols {
                    return Err("invalid_cell");
                };
                let id = r * cols + c;
                match a["type"].as_str() {
                    Some("flag") => {
                        if seen[id] {
                            return Err("revealed_flag");
                        };
                        flags[id] = !flags[id]
                    }
                    Some("reveal") => {
                        if seen[id] || flags[id] {
                            return Err("invalid_reveal");
                        };
                        if !placed {
                            let mut n = 0;
                            while n < count {
                                let rr = rng.index(rows);
                                let cc = rng.index(cols);
                                let j = rr * cols + cc;
                                if !mines[j] && (rr.abs_diff(r) > 1 || cc.abs_diff(c) > 1) {
                                    mines[j] = true;
                                    n += 1
                                }
                            }
                            placed = true;
                        }
                        if mines[id] {
                            return Err("lost_game");
                        };
                        let mut queue = vec![(r, c)];
                        while let Some((rr, cc)) = queue.pop() {
                            let j = rr * cols + cc;
                            if seen[j] || flags[j] || mines[j] {
                                continue;
                            };
                            seen[j] = true;
                            let neighbors: Vec<(usize, usize)> = (-1isize..=1)
                                .flat_map(|dr| (-1isize..=1).map(move |dc| (dr, dc)))
                                .filter_map(|(dr, dc)| {
                                    let nr = rr as isize + dr;
                                    let nc = cc as isize + dc;
                                    if (dr != 0 || dc != 0)
                                        && nr >= 0
                                        && nc >= 0
                                        && nr < rows as isize
                                        && nc < cols as isize
                                    {
                                        Some((nr as usize, nc as usize))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if !neighbors.iter().any(|&(nr, nc)| mines[nr * cols + nc]) {
                                queue.extend(neighbors)
                            }
                        }
                    }
                    _ => return Err("invalid_action"),
                }
            }
            if seen.iter().filter(|&&v| v).count() != rows * cols - count {
                return Err("incomplete_game");
            };
            Ok(1_000_000 - actions.len() as i64)
        }
        _ => crate::action_games::replay(game, seed, params, actions),
    }
}
fn spawn(board: &mut [u32; 16], rng: &mut Rng) {
    let empty: Vec<usize> = (0..16).filter(|&i| board[i] == 0).collect();
    if !empty.is_empty() {
        let i = empty[rng.index(empty.len())];
        board[i] = if rng.next() < 0.9 { 2 } else { 4 }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn deterministic() {
        assert_eq!(
            replay("2048", 123, &json!({}), &[json!("left")]),
            replay("2048", 123, &json!({}), &[json!("left")])
        );
    }
    #[test]
    fn invalid() {
        assert!(replay("2048", 1, &json!({}), &[json!("cheat")]).is_err());
        assert!(replay("memory", 1, &json!({}), &[json!(0), json!(0)]).is_err());
        assert!(
            replay(
                "mines",
                1,
                &json!({}),
                &[json!({"type":"reveal","r":99,"c":0})]
            )
            .is_err()
        );
    }
    #[test]
    fn memory_complete() {
        let mut rng = Rng(42);
        let mut cards: Vec<usize> = (0..6).chain(0..6).collect();
        for i in (1..12).rev() {
            let j = rng.index(i + 1);
            cards.swap(i, j)
        }
        let actions: Vec<Value> = (0..6)
            .flat_map(|v| {
                cards
                    .iter()
                    .enumerate()
                    .filter(move |(_, c)| **c == v)
                    .map(|(i, _)| json!(i))
            })
            .collect();
        assert_eq!(replay("memory", 42, &json!({}), &actions), Ok(999994));
    }
}
