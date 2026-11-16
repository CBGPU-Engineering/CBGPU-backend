//! Deadline-driven, transactionally idempotent allocation. No HTTP listener or admin credentials.
use rand::seq::SliceRandom;
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Every entrant is assigned exactly once, within the host's inclusive range.
/// Impossible counts stay unassigned instead of silently breaking the host's rules.
pub fn balanced_sizes(count: usize, min: usize, max: usize) -> Option<Vec<usize>> {
    if min == 0 || min > max || max > 50 {
        return None;
    }
    if count == 0 {
        return Some(vec![]);
    }
    let teams = count.div_ceil(max);
    if count / teams < min {
        return None;
    }
    Some(
        (0..teams)
            .map(|i| count / teams + usize::from(i < count % teams))
            .collect(),
    )
}

#[derive(Default, Debug, Serialize)]
pub struct Report {
    pub contests: usize,
    pub created_teams: usize,
    pub assigned: usize,
    pub waiting: usize,
}

pub async fn allocate(pool: &PgPool, dry_run: bool) -> Result<Report, sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM public.contests WHERE participation_mode='team' AND lifecycle_status='published' AND coalesce(registration_closes_at,event_start_at)<=clock_timestamp() ORDER BY id").fetch_all(pool).await?;
    let mut report = Report::default();
    for id in ids {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL lock_timeout='20s'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='30s'")
            .execute(&mut *tx)
            .await?;
        // Shared with registration RPC and host settings trigger: two workers never split the same queue.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        let Some(settings) = sqlx::query("SELECT team_min_size,team_max_size,allow_individual_entries FROM public.contests WHERE id=$1 AND participation_mode='team' AND lifecycle_status='published' AND coalesce(registration_closes_at,event_start_at)<=clock_timestamp()")
            .bind(id).fetch_optional(&mut *tx).await? else { tx.rollback().await?; continue; };
        report.contests += 1;
        let min: i32 = settings.try_get("team_min_size")?;
        let max: i32 = settings.try_get("team_max_size")?;
        sqlx::query("UPDATE public.contest_teams t SET formation_status=CASE WHEN (SELECT count(*) FROM public.contest_team_members m WHERE m.team_id=t.id AND m.registration_status='confirmed') BETWEEN $2 AND $3 THEN 'ready' ELSE 'needs_members' END WHERE t.contest_id=$1")
            .bind(id).bind(min).bind(max).execute(&mut *tx).await?;
        if settings.try_get::<bool, _>("allow_individual_entries")? {
            let mut entrants = sqlx::query("SELECT r.id,r.user_id,r.attendee_name FROM public.event_registrations r WHERE r.contest_id=$1 AND r.status='confirmed' AND r.participation_kind='individual' AND NOT EXISTS(SELECT 1 FROM public.contest_team_members m WHERE m.contest_id=r.contest_id AND m.user_id=r.user_id) ORDER BY r.user_id")
                .bind(id).fetch_all(&mut *tx).await?;
            if let Some(sizes) = balanced_sizes(entrants.len(), min as usize, max as usize) {
                entrants.shuffle(&mut rand::thread_rng());
                let mut offset = 0;
                for size in sizes {
                    let team = Uuid::new_v4();
                    let name = format!("Random team {}", &team.to_string()[..8]);
                    sqlx::query("INSERT INTO public.contest_teams(id,contest_id,name,kind,formation_status) VALUES($1,$2,$3,'random','ready')")
                        .bind(team).bind(id).bind(name).execute(&mut *tx).await?;
                    for entrant in &entrants[offset..offset + size] {
                        sqlx::query("INSERT INTO public.contest_team_members(contest_id,team_id,user_id,registration_id,attendee_name,registration_status) VALUES($1,$2,$3,$4,$5,'confirmed')")
                            .bind(id).bind(team).bind(entrant.try_get::<Uuid,_>("user_id")?).bind(entrant.try_get::<Uuid,_>("id")?).bind(entrant.try_get::<String,_>("attendee_name")?).execute(&mut *tx).await?;
                    }
                    offset += size;
                    report.created_teams += 1;
                    report.assigned += size;
                }
            } else {
                report.waiting += entrants.len();
            }
        }
        if dry_run {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_feasible_counts_preserve_every_entrant_and_host_bounds() {
        for min in 1..=50 {
            for max in min..=50 {
                for n in 0..=500 {
                    let result = balanced_sizes(n, min, max);
                    let feasible = n == 0 || (1..=n).any(|k| k * min <= n && n <= k * max);
                    assert_eq!(result.is_some(), feasible, "n={n}, range={min}..{max}");
                    if let Some(sizes) = result {
                        assert_eq!(sizes.iter().sum::<usize>(), n);
                        assert!(sizes.iter().all(|s| (min..=max).contains(s)));
                        if let (Some(a), Some(b)) = (sizes.iter().min(), sizes.iter().max()) {
                            assert!(b - a <= 1);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn no_silent_underfilled_or_oversized_teams() {
        assert_eq!(balanced_sizes(5, 3, 4), None);
        assert_eq!(balanced_sizes(7, 3, 4), Some(vec![4, 3]));
        assert_eq!(balanced_sizes(2, 0, 4), None);
        assert_eq!(balanced_sizes(2, 2, 51), None);
    }
}
